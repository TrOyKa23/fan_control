#![no_std]
#![no_main]

use defmt::*;
use defmt_rtt as _;
use panic_probe as _;

use embassy_executor::Spawner;
use embassy_rp::gpio::{Input, Level, Output, Pull};
use embassy_rp::pwm::{Config as PwmConfig, Pwm};
use embassy_time::{Duration, Instant, Timer};

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    info!("=== Запуск прошивки ===");

    let p = embassy_rp::init(Default::default());
    info!("Супервизор TPA3255 запущен!");

    // --- Инициализация GPIO ---
    let btn_power = Input::new(p.PIN_2, Pull::Up); // Пин 4 (GPIO 2)
    let fault_bus = Input::new(p.PIN_7, Pull::Up); // Пин 10 (GPIO 7)

    let mut led_power = Output::new(p.PIN_3, Level::Low); // Пин 5 (GPIO 3)
    let mut led_fault = Output::new(p.PIN_4, Level::Low); // Пин 6 (GPIO 4)
    let mut en_5v_main = Output::new(p.PIN_5, Level::Low); // Пин 7 (GPIO 5) — Ключ питания Q6/Q7
    let mut relay_en = Output::new(p.PIN_6, Level::Low); // Пин 9 (GPIO 6) — Реле 230 В
    let mut fan_en = Output::new(p.PIN_16, Level::Low); // Пин 21 (GPIO 16) — Вентиляторы

    // --- Настройка PWM кулера (Пин 22 / GPIO 17) ---
    let mut pwm_config = PwmConfig::default();
    pwm_config.top = 255;
    pwm_config.compare_b = 128;
    let _fan_pwm = Pwm::new_output_b(p.PWM_SLICE0, p.PIN_17, pwm_config.clone());

    let mut system_active = false;

    let mut last_button_press: Option<Instant> = None;

    loop {
        // Проверка нажатия кнопки (Пин 4 -> GND)
        if btn_power.is_low() {
            Timer::after(Duration::from_millis(1000)).await;

            if btn_power.is_low() {
                let now = Instant::now();

                let can_press = match last_button_press {
                    Some(last) => now - last >= Duration::from_secs(2),
                    None => true,
                };

                if can_press {
                    // Запоминаем время принятого нажатия
                    last_button_press = Some(now);

                    system_active = !system_active;
                    info!("Кнопка POWER нажата!");

                    if system_active {
                        info!("Включение системы...");
                        led_power.set_high();
                        en_5v_main.set_high(); // ВКЛЮЧАЕМ силовые +5V_MAIN (Q7/Q6)
                        relay_en.set_high(); // ВКЛЮЧАЕМ реле K3
                        fan_en.set_high(); // ВКЛЮЧАЕМ питание кулеров
                    } else {
                        info!("Выключение системы...");
                        led_power.set_low();
                        en_5v_main.set_low(); // ОТКЛЮЧАЕМ +5V_MAIN
                        relay_en.set_low(); // ОТКЛЮЧАЕМ реле K3
                        fan_en.set_low(); // ОТКЛЮЧАЕМ питание кулеров
                    }
                } else {
                    info!("Нажатие POWER проигнорировано: интервал менее 2 секунд!");
                }

                // Ждём отпускания кнопки, чтобы удержание не считалось новым нажатием
                while btn_power.is_low() {
                    Timer::after(Duration::from_millis(10)).await;
                }
            }
        }

        // Защита по ошибке FAULT_BUS
        if system_active && fault_bus.is_low() {
            error!("Зафиксирована ошибка (FAULT)! Экстренное отключение!");

            en_5v_main.set_low();
            relay_en.set_low();
            led_power.set_low();
            led_fault.set_high();
            system_active = false;

            for _ in 0..10 {
                led_fault.toggle();
                Timer::after(Duration::from_millis(200)).await;
            }
            led_fault.set_high();
        }

        Timer::after(Duration::from_millis(20)).await;
    }
}
