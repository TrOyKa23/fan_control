#![no_std]
#![no_main]

mod ds18b20;
mod fan;
mod network;
mod state;

use defmt::*;
use defmt_rtt as _;
use panic_probe as _;

use ds18b20::OneWireBus;
use embassy_executor::Spawner;
use embassy_rp::gpio::{Input, Level, Output, Pull};
use embassy_rp::pwm::{Config as PwmConfig, Pwm};
use embassy_time::{Duration, Instant, Timer};

#[embassy_executor::task]
async fn tach_monitor_task(mut tach_pin: Input<'static>) {
    loop {
        let start = Instant::now();
        let mut pulse_count: u32 = 0;

        while start.elapsed() < Duration::from_secs(1) {
            match embassy_time::with_timeout(
                Duration::from_millis(100),
                tach_pin.wait_for_falling_edge(),
            )
            .await
            {
                Ok(_) => {
                    pulse_count += 1;
                }
                Err(_) => {}
            }
        }

        let rpm = pulse_count * 30;
        crate::state::set_rpm(rpm);
        info!("Скорость вентилятора: {} RPM", rpm);
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    info!("Супервизор TPA3255 запущен!");

    let btn_power = Input::new(p.PIN_2, Pull::Up);
    let fault_bus = Input::new(p.PIN_7, Pull::Up);
    let tach_pin = Input::new(p.PIN_19, Pull::Up);

    // .unwrap() внутри: извлекаем SpawnToken
    spawner.spawn(tach_monitor_task(tach_pin).unwrap());

    // Передаем p.PIN_25 (WL_CS на Pico W)
    network::spawn_network_task(
        &spawner, p.PIN_23, p.PIO0, p.PIN_24, p.PIN_29, p.DMA_CH0, p.PIN_25,
    );

    let mut led_power = Output::new(p.PIN_3, Level::Low);
    let mut led_fault = Output::new(p.PIN_4, Level::Low);
    let mut en_5v_main = Output::new(p.PIN_5, Level::Low);
    let mut relay_en = Output::new(p.PIN_6, Level::Low);
    let mut fan_en = Output::new(p.PIN_16, Level::Low);

    let mut pwm_config = PwmConfig::default();
    pwm_config.top = 5999;
    pwm_config.compare_b = 0;
    let mut fan_pwm = Pwm::new_output_b(p.PWM_SLICE0, p.PIN_17, pwm_config.clone());

    let mut ow_bus = OneWireBus::new(embassy_rp::gpio::Flex::new(p.PIN_22));

    let mut sensor_roms = [[0u8; 8]; 6];
    info!("Сканирование шины 1-Wire...");
    let sensor_count = ow_bus.search_rom(&mut sensor_roms);
    info!("Успешно найдено уникальных датчиков: {}", sensor_count);

    for i in 0..sensor_count {
        info!("Д{}: ID = {:x}", i + 1, sensor_roms[i]);
    }

    let mut system_active = false;
    let mut max_temp_deci_c: i16 = 200;

    loop {
        if sensor_count > 0 {
            if ow_bus.reset() {
                ow_bus.write_byte(0xCC);
                ow_bus.write_byte(0x44);
                Timer::after(Duration::from_millis(750)).await;

                let mut current_max = -500;

                for i in 0..sensor_count {
                    if ow_bus.reset() {
                        ow_bus.match_rom(&sensor_roms[i]);
                        ow_bus.write_byte(0xBE);

                        let lsb = ow_bus.read_byte();
                        let msb = ow_bus.read_byte();
                        let raw_temp = ((msb as i16) << 8) | (lsb as i16);

                        let temp_deci = (raw_temp * 10) / 16;
                        info!("Д{}: {}.{}°C", i + 1, temp_deci / 10, temp_deci % 10);

                        crate::state::set_temp(i, temp_deci);

                        if temp_deci > current_max {
                            current_max = temp_deci;
                        }
                    }
                }

                if current_max > -500 {
                    max_temp_deci_c = current_max;
                    crate::state::set_temp(0, max_temp_deci_c);
                }
            }
        }

        if system_active {
            let compare_val = fan::calculate_pwm_compare(max_temp_deci_c);
            pwm_config.compare_b = compare_val;
            fan_pwm.set_config(&pwm_config);
        }

        if system_active && max_temp_deci_c >= 800 {
            error!("ПЕРЕГРЕВ! T_max >= 80°C! Аварийное отключение!");
            en_5v_main.set_low();
            relay_en.set_low();
            led_power.set_low();
            led_fault.set_high();
            system_active = false;
            pwm_config.compare_b = 0;
            fan_pwm.set_config(&pwm_config);
        }

        if btn_power.is_low() {
            Timer::after(Duration::from_millis(50)).await;
            if btn_power.is_low() {
                system_active = !system_active;
                crate::state::set_system_active(system_active);

                if system_active {
                    info!("Включение системы...");
                    led_power.set_high();
                    en_5v_main.set_high();
                    relay_en.set_high();
                    fan_en.set_high();
                } else {
                    info!("Выключение системы...");
                    led_power.set_low();
                    en_5v_main.set_low();
                    relay_en.set_low();
                    fan_en.set_low();
                    pwm_config.compare_b = 0;
                    fan_pwm.set_config(&pwm_config);
                }

                while btn_power.is_low() {
                    Timer::after(Duration::from_millis(10)).await;
                }
            }
        }

        if system_active && fault_bus.is_low() {
            error!("Зафиксирована ошибка FAULT_BUS! Экстренное отключение!");
            en_5v_main.set_low();
            relay_en.set_low();
            led_power.set_low();
            led_fault.set_high();
            system_active = false;
            pwm_config.compare_b = 0;
            fan_pwm.set_config(&pwm_config);
        }

        Timer::after(Duration::from_millis(100)).await;
    }
}
