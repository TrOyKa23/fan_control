use cyw43_pio::PioSpi;
use defmt::{error, info, warn};
use embassy_executor::Spawner;
use embassy_net::{Stack, StackResources};
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIN_23, PIN_24, PIN_25, PIN_29, PIO0};
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_rp::Peri;
use embassy_time::{Duration, Timer};
use picoserve::routing::get;
use serde::Serialize;
use static_cell::StaticCell;

embassy_rp::bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
    DMA_IRQ_0 => embassy_rp::dma::InterruptHandler<DMA_CH0>;
});

#[derive(Serialize, Clone, Copy)]
struct Telemetry {
    temps: [i16; 6],
    rpm: u32,
    system_active: bool,
}

// Корректное 4-байтовое выравнивание бинарных прошивок в Flash
#[repr(C, align(4))]
struct Aligned<T: ?Sized>(T);

static FW_DATA: Aligned<[u8; include_bytes!("firmware/43439A0.bin").len()]> =
    Aligned(*include_bytes!("firmware/43439A0.bin"));
// Стандартная строка NVRAM для Pico W / Pico 2 W с авто-подсчетом длины
const NVRAM_BYTES: &[u8; 289] = b"NVRAMRev=$Rev: 7.95.49.00 $\0\
manfid=0x2d0\0\
prodid=0x0727\0\
vendid=0x14e4\0\
devid=0x43a9\0\
boardtype=0x0727\0\
boardrev=0x1102\0\
boardnum=16023\0\
macaddr=00:A0:50:b5:59:5e\0\
sromrev=11\0\
boardflags=0x00404201\0\
xtalfreq=37400\0\
nocrc=1\0\
ag0=255\0\
aa2g=1\0\
ccode=ALL\0\
pa0itssit=0x20\0\
extpagain2g=0\0\
pa2gw3a=0x0000\0\
radv=0\0\0";

static NVRAM_DATA: Aligned<[u8; NVRAM_BYTES.len()]> = Aligned(*NVRAM_BYTES);

pub fn spawn_network_task(
    spawner: &Spawner,
    pwr: Peri<'static, PIN_23>,
    pio: Peri<'static, PIO0>,
    dio: Peri<'static, PIN_24>,
    clk: Peri<'static, PIN_29>,
    dma: Peri<'static, DMA_CH0>,
    cs: Peri<'static, PIN_25>,
) {
    spawner.spawn(wifi_stack_task(*spawner, pwr, pio, dio, clk, dma, cs).unwrap());
}

#[embassy_executor::task]
async fn wifi_stack_task(
    spawner: Spawner,
    pwr_pin: Peri<'static, PIN_23>,
    pio_periph: Peri<'static, PIO0>,
    dio: Peri<'static, PIN_24>,
    clk: Peri<'static, PIN_29>,
    dma: Peri<'static, DMA_CH0>,
    cs_pin: Peri<'static, PIN_25>,
) {
    let pwr = Output::new(pwr_pin, Level::Low);
    let cs = Output::new(cs_pin, Level::High);
    let mut pio = Pio::new(pio_periph, Irqs);
    let dma_ch = embassy_rp::dma::Channel::new(dma, Irqs);

    // Делитель RM2 специально для Pico 2 W
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        cyw43_pio::RM2_CLOCK_DIVIDER,
        pio.irq0,
        cs,
        dio,
        clk,
        dma_ch,
    );

    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = STATE.init(cyw43::State::new());

    let fw: &'static cyw43::Aligned<cyw43::A4, [u8]> = unsafe {
        &*(FW_DATA.0.as_slice() as *const [u8] as *const cyw43::Aligned<cyw43::A4, [u8]>)
    };
    let nvram: &'static cyw43::Aligned<cyw43::A4, [u8]> = unsafe {
        &*(NVRAM_DATA.0.as_slice() as *const [u8] as *const cyw43::Aligned<cyw43::A4, [u8]>)
    };

    // Передаем fw и nvram
    let (net_device, mut control, runner) = cyw43::new(state, pwr, spi, fw, nvram).await;

    spawner.spawn(cyw43_task(runner).unwrap());

    control
        .set_power_management(cyw43::PowerManagementMode::PowerSave)
        .await;

    // ВНИМАНИЕ: укажите данные вашей Wi-Fi сети
    let ssid = "DIGI-nEh3";
    let password = "hxmsznr3Tp";

    spawner.spawn(wifi_join_task(control, ssid, password).unwrap());

    static STACK_RESOURCES: StaticCell<StackResources<5>> = StaticCell::new();
    let resources = STACK_RESOURCES.init(StackResources::new());

    let seed = 0x0123_4567_89ab_cdef;
    let config = embassy_net::Config::dhcpv4(Default::default());

    let (stack, runner) = embassy_net::new(net_device, config, resources, seed);

    spawner.spawn(net_task(runner).unwrap());
    spawner.spawn(http_server_task(stack).unwrap());
}

#[embassy_executor::task]
async fn cyw43_task(
    runner: cyw43::Runner<'static, cyw43::SpiBus<Output<'static>, PioSpi<'static, PIO0, 0>>>,
) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn wifi_join_task(
    mut control: cyw43::Control<'static>,
    ssid: &'static str,
    password: &'static str,
) -> ! {
    loop {
        info!("Подключение к Wi-Fi: {}...", ssid);
        match control
            .join(ssid, cyw43::JoinOptions::new(password.as_bytes()))
            .await
        {
            Ok(_) => {
                info!("Wi-Fi подключен успешно!");
                break;
            }
            Err(err) => {
                error!(
                    "Ошибка подключения к Wi-Fi: {:?}. Повтор через 5 секунд...",
                    err
                );
                Timer::after(Duration::from_secs(5)).await;
            }
        }
    }

    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}

#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, cyw43::NetDriver<'static>>) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn http_server_task(stack: Stack<'static>) -> ! {
    let mut rx_buffer = [0u8; 1024];
    let mut tx_buffer = [0u8; 1024];
    let mut http_buffer = [0u8; 2048];

    info!("Ожидание получения IP-адреса по DHCP...");
    stack.wait_config_up().await;

    if let Some(config) = stack.config_v4() {
        info!("Сеть готова! Назначенный IP: {:?}", config.address);
        info!("=======================================================");
        info!("ОТКРОЙТЕ В БРАУЗЕРЕ: http://{}/", config.address.address());
        info!("=======================================================");
    }

    let router = picoserve::Router::new()
        .route(
            "/",
            get(|| async {
                picoserve::response::json::Json(Telemetry {
                    temps: crate::state::get_temps(),
                    rpm: crate::state::get_rpm(),
                    system_active: crate::state::get_system_active(),
                })
            }),
        )
        .route(
            "/telemetry",
            get(|| async {
                picoserve::response::json::Json(Telemetry {
                    temps: crate::state::get_temps(),
                    rpm: crate::state::get_rpm(),
                    system_active: crate::state::get_system_active(),
                })
            }),
        );

    let config = picoserve::Config::const_default();

    loop {
        let mut socket = embassy_net::tcp::TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
        socket.set_timeout(Some(Duration::from_secs(10)));

        if let Err(e) = socket.accept(80).await {
            warn!("TCP accept error: {:?}", e);
            continue;
        }

        let _ = picoserve::Server::new(&router, &config, &mut http_buffer)
            .serve(socket)
            .await;
    }
}
