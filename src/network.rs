use cyw43_pio::PioSpi;
use defmt::{error, info, warn};
use embassy_executor::Spawner;
use embassy_net::{Stack, StackResources};
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIN_23, PIN_24, PIN_25, PIN_29, PIO0};
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_rp::Peri;
use embassy_time::{Duration, Timer};
use picoserve::routing::{get, post};
use serde::Deserialize;
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

#[derive(Deserialize, Clone, Copy)]
struct FanCommand {
    speed: u8, // 0 - авто, 1..100 - ручная скорость в процентах
}

// Корректное 4-байтовое выравнивание бинарных прошивок в Flash
#[repr(C, align(4))]
struct Aligned<T: ?Sized>(T);

static FW_DATA: Aligned<[u8; include_bytes!("firmware/43439A0.bin").len()]> =
    Aligned(*include_bytes!("firmware/43439A0.bin"));

static CLM_DATA: Aligned<[u8; include_bytes!("firmware/43439A0_clm.bin").len()]> =
    Aligned(*include_bytes!("firmware/43439A0_clm.bin"));

// Стандартный шаблон NVRAM-строки без жесткого MAC-адреса
const NVRAM_TEMPLATE: &[u8] = b"NVRAMRev=$Rev: 7.95.49.00 $\0\
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
    dio_pin: Peri<'static, PIN_24>,
    clk_pin: Peri<'static, PIN_29>,
    dma_ch_periph: Peri<'static, DMA_CH0>,
    cs_pin: Peri<'static, PIN_25>,
) {
    let pwr = Output::new(pwr_pin, Level::Low);
    let cs = Output::new(cs_pin, Level::High);
    let mut pio = Pio::new(pio_periph, Irqs);
    let dma_ch = embassy_rp::dma::Channel::new(dma_ch_periph, Irqs);

    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        cyw43_pio::RM2_CLOCK_DIVIDER,
        pio.irq0,
        cs,
        dio_pin,
        clk_pin,
        dma_ch,
    );

    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = STATE.init(cyw43::State::new());

    // Генерация динамического MAC-адреса
    let seed = embassy_time::Instant::now().as_micros();
    let mac_bytes: [u8; 6] = [
        0x02,
        ((seed >> 8) & 0xFF) as u8,
        ((seed >> 16) & 0xFF) as u8,
        ((seed >> 24) & 0xFF) as u8,
        ((seed >> 32) & 0xFF) as u8,
        (seed & 0xFF) as u8,
    ];

    let mut nvram_buf = [0u8; 289];
    nvram_buf.copy_from_slice(NVRAM_TEMPLATE);

    if let Some(pos) = nvram_buf.windows(8).position(|w| w == b"macaddr=") {
        let start = pos + 8;
        let mut mac_str = [0u8; 17];
        let hex_chars = b"0123456789abcdef";

        for (i, &b) in mac_bytes.iter().enumerate() {
            let idx = i * 3;
            mac_str[idx] = hex_chars[(b >> 4) as usize];
            mac_str[idx + 1] = hex_chars[(b & 0x0f) as usize];
            if i < 5 {
                mac_str[idx + 2] = b':';
            }
        }

        nvram_buf[start..start + 17].copy_from_slice(&mac_str);
        info!(
            "Динамический MAC-адрес Wi-Fi: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac_bytes[0], mac_bytes[1], mac_bytes[2], mac_bytes[3], mac_bytes[4], mac_bytes[5]
        );
    }

    static NVRAM_DATA_CELL: StaticCell<Aligned<[u8; 289]>> = StaticCell::new();
    let nvram_data = NVRAM_DATA_CELL.init(Aligned(nvram_buf));

    let fw: &'static cyw43::Aligned<cyw43::A4, [u8]> = unsafe {
        &*(FW_DATA.0.as_slice() as *const [u8] as *const cyw43::Aligned<cyw43::A4, [u8]>)
    };
    let nvram: &'static cyw43::Aligned<cyw43::A4, [u8]> = unsafe {
        &*(nvram_data.0.as_slice() as *const [u8] as *const cyw43::Aligned<cyw43::A4, [u8]>)
    };
    let clm: &'static cyw43::Aligned<cyw43::A4, [u8]> = unsafe {
        &*(CLM_DATA.0.as_slice() as *const [u8] as *const cyw43::Aligned<cyw43::A4, [u8]>)
    };

    let (net_device, mut control, runner) = cyw43::new(state, pwr, spi, fw, nvram).await;

    spawner.spawn(cyw43_task(runner).unwrap());

    control.init(clm).await;
    control
        .set_power_management(cyw43::PowerManagementMode::PowerSave)
        .await;

    let ssid = "DIGI-nEh3";
    let password = "hxmsznr3Tp";

    spawner.spawn(wifi_join_task(control, ssid, password).unwrap());

    static STACK_RESOURCES: StaticCell<StackResources<5>> = StaticCell::new();
    let resources = STACK_RESOURCES.init(StackResources::new());

    let net_seed = 0x0123_4567_89ab_cdef;
    let config = embassy_net::Config::dhcpv4(Default::default());

    let (stack, runner) = embassy_net::new(net_device, config, resources, net_seed);

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
            picoserve::routing::get_service(picoserve::response::fs::File::html(
                r#"<!DOCTYPE html>
                <html>
                <head>
                    <meta charset="utf-8">
                    <title>Fan Control TPA3255</title>
                    <style>
                        body { font-family: Arial, sans-serif; text-align: center; margin-top: 50px; background: #121212; color: #e0e0e0; }
                        .card { background: #1e1e1e; padding: 20px; border-radius: 10px; display: inline-block; box-shadow: 0 4px 10px rgba(0,0,0,0.5); }
                        .val { color: #4caf50; font-weight: bold; }
                    </style>
                </head>
                <body>
                    <div class="card">
                        <h2>Телеметрия системы</h2>
                        <p>Вентилятор: <span id="rpm" class="val">--</span> RPM</p>
                        <p>Датчики 1-6: <span id="temps" class="val">--</span> °C</p>
                        <p>Статус: <span id="active" class="val">--</span></p>
                    </div>
                    
                    <div class="card">
                        <h2>Управление вентилятором</h2>
                        <p id="mode-text">Режим: Авто</p>
                        <input type="range" id="fan-slider" min="0" max="100" value="0" oninput="onSliderChange(this.value)">
                    </div>

                    <script>
                        function onSliderChange(val) {
                            let modeText = document.getElementById('mode-text');
                            if (val == 0) {
                                modeText.innerText = "Режим: Авто";
                            } else {
                                modeText.innerText = "Режим: Ручной (" + val + "%)";
                            }
                            fetch('/set_fan', {
                                method: 'POST',
                                headers: { 'Content-Type': 'application/json' },
                                body: JSON.stringify({ speed: parseInt(val) })
                            }).catch(e => console.error('Ошибка отправки', e));
                        }

                        async function updateData() {
                            try {
                                let response = await fetch('/telemetry');
                                let data = await response.json();
                                document.getElementById('rpm').innerText = data.rpm;
                                let formattedTemps = data.temps.map(t => (t / 10).toFixed(1)).join(', ');
                                document.getElementById('temps').innerText = formattedTemps;
                                document.getElementById('active').innerText = data.system_active ? 'Активен' : 'Остановлен';
                            } catch (e) {
                                console.error('Ошибка получения данных', e);
                            }
                        }
                        setInterval(updateData, 1000);
                        updateData();
                    </script>
                </body>
                </html>"#,
            )),
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
        )
        .route(
            "/set_fan",
            post(|picoserve::extract::Json(cmd): picoserve::extract::Json<FanCommand>| async move {
                crate::state::set_fan_override(cmd.speed);
                "OK"
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
