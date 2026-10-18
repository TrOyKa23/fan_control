#![no_std]
#![no_main]

use defmt::*;
use defmt_rtt as _;
use panic_probe as _;

use embassy_executor::Spawner;
use embassy_rp::gpio::{Flex, Input, Level, Output, Pull};
use embassy_rp::pwm::{Config as PwmConfig, Pwm};
use embassy_time::{Duration, Timer};

/// В ARM Cortex-M33 цикл delay() занимает 2 такта.
/// При частоте 150 МГц: 150 / 2 = 75 итераций на 1 микросекунду.
const CYCLES_PER_MICROS: u32 = 75;

#[inline(always)]
fn delay_us(us: u32) {
    cortex_m::asm::delay(us * CYCLES_PER_MICROS);
}

struct OneWireBus<'a> {
    pin: Flex<'a>,
}

impl<'a> OneWireBus<'a> {
    pub fn new(mut pin: Flex<'a>) -> Self {
        pin.set_as_input();
        pin.set_pull(Pull::Up);
        Self { pin }
    }

    pub fn reset(&mut self) -> bool {
        self.pin.set_as_output();
        self.pin.set_low();
        delay_us(480);

        self.pin.set_as_input();
        delay_us(70);

        let presence = self.pin.is_low();
        delay_us(410);
        presence
    }

    pub fn write_bit(&mut self, bit: bool) {
        self.pin.set_as_output();
        self.pin.set_low();
        if bit {
            delay_us(6);
            self.pin.set_as_input();
            delay_us(64);
        } else {
            delay_us(60);
            self.pin.set_as_input();
            delay_us(10);
        }
    }

    pub fn read_bit(&mut self) -> bool {
        self.pin.set_as_output();
        self.pin.set_low();
        delay_us(2);

        self.pin.set_as_input();
        delay_us(10);

        let bit = self.pin.is_high();
        delay_us(50);
        bit
    }

    pub fn write_byte(&mut self, mut byte: u8) {
        for _ in 0..8 {
            self.write_bit((byte & 0x01) != 0);
            byte >>= 1;
        }
    }

    pub fn read_byte(&mut self) -> u8 {
        let mut byte = 0u8;
        for i in 0..8 {
            if self.read_bit() {
                byte |= 1 << i;
            }
        }
        byte
    }

    pub fn match_rom(&mut self, rom: &[u8; 8]) {
        self.write_byte(0x55);
        for &byte in rom {
            self.write_byte(byte);
        }
    }

    /// Полноценный алгоритм Search ROM по спецификации Maxim AN187
    pub fn search_rom(&mut self, roms: &mut [[u8; 8]; 6]) -> usize {
        let mut count = 0;
        let mut last_discrepancy = 0i8;
        let mut last_device_flag = false;
        let mut last_rom = [0u8; 8];

        while !last_device_flag && count < 6 {
            if !self.reset() {
                break;
            }

            self.write_byte(0xF0); // Команда Search ROM
            let mut last_zero = 0i8;
            let mut rom_byte_index = 0usize;
            let mut rom_byte_mask = 1u8;
            let mut current_rom = [0u8; 8];
            let mut search_result = true;

            for bit_number in 1..=64i8 {
                let id_bit = self.read_bit();
                let cmp_id_bit = self.read_bit();

                // 11 = Ошибка на шине (нет ответа)
                if id_bit && cmp_id_bit {
                    search_result = false;
                    break;
                }

                let search_direction = if !id_bit && !cmp_id_bit {
                    // Коллизия: присутствуют устройства и с 0, и с 1
                    let dir = if bit_number == last_discrepancy {
                        true
                    } else if bit_number > last_discrepancy {
                        false
                    } else {
                        (last_rom[rom_byte_index] & rom_byte_mask) != 0
                    };

                    // Запоминаем развилку ТОЛЬКО если выбрали 0
                    if !dir {
                        last_zero = bit_number;
                    }
                    dir
                } else {
                    // Все устройства имеют одинаковое значение бита
                    id_bit
                };

                if search_direction {
                    current_rom[rom_byte_index] |= rom_byte_mask;
                } else {
                    current_rom[rom_byte_index] &= !rom_byte_mask;
                }

                self.write_bit(search_direction);

                rom_byte_mask <<= 1;
                if rom_byte_mask == 0 {
                    rom_byte_index += 1;
                    rom_byte_mask = 1;
                }
            }

            if search_result {
                last_discrepancy = last_zero;
                if last_discrepancy == 0 {
                    last_device_flag = true;
                }
                last_rom = current_rom;
                roms[count] = current_rom;
                count += 1;
            } else {
                break;
            }
        }
        count
    }
}

fn calculate_pwm_compare(temp_deci_c: i16) -> u16 {
    const TOP: u32 = 5999;
    const TEMP_MIN: i16 = 250; // 24.0 °C
    const TEMP_MAX: i16 = 255; // 26.0 °C

    if temp_deci_c <= TEMP_MIN {
        0
    } else if temp_deci_c >= TEMP_MAX {
        (TOP + 1) as u16
    } else {
        let t_diff = (temp_deci_c - TEMP_MIN) as u32;
        let temp_range = (TEMP_MAX - TEMP_MIN) as u32;
        let max_pwm = TOP + 1;

        ((t_diff * max_pwm) / temp_range) as u16
    }
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    info!("Супервизор TPA3255 запущен!");

    let btn_power = Input::new(p.PIN_2, Pull::Up); // GP2
    let fault_bus = Input::new(p.PIN_7, Pull::Up); // GP7

    let mut led_power = Output::new(p.PIN_3, Level::Low); // GP3
    let mut led_fault = Output::new(p.PIN_4, Level::Low); // GP4
    let mut en_5v_main = Output::new(p.PIN_5, Level::Low); // GP5
    let mut relay_en = Output::new(p.PIN_6, Level::Low); // GP6
    let mut fan_en = Output::new(p.PIN_16, Level::Low); // GP16

    let mut pwm_config = PwmConfig::default();
    pwm_config.top = 5999;
    pwm_config.compare_b = 900;
    let mut fan_pwm = Pwm::new_output_b(p.PWM_SLICE0, p.PIN_17, pwm_config.clone());

    let mut ow_bus = OneWireBus::new(Flex::new(p.PIN_22));

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
                ow_bus.write_byte(0xCC); // Skip ROM (Запуск измерения на всех)
                ow_bus.write_byte(0x44);
                Timer::after(Duration::from_millis(750)).await;

                let mut current_max = -500;

                for i in 0..sensor_count {
                    if ow_bus.reset() {
                        ow_bus.match_rom(&sensor_roms[i]); // Адресный запрос
                        ow_bus.write_byte(0xBE);

                        let lsb = ow_bus.read_byte();
                        let msb = ow_bus.read_byte();
                        let raw_temp = ((msb as i16) << 8) | (lsb as i16);

                        let temp_deci = (raw_temp * 10) / 16;
                        info!("Д{}: {}.{}°C", i + 1, temp_deci / 10, temp_deci % 10);

                        if temp_deci > current_max {
                            current_max = temp_deci;
                        }
                    }
                }

                if current_max > -500 {
                    max_temp_deci_c = current_max;
                }
            }
        }

        if system_active {
            let compare_val = calculate_pwm_compare(max_temp_deci_c);
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
        }

        if btn_power.is_low() {
            Timer::after(Duration::from_millis(50)).await;
            if btn_power.is_low() {
                system_active = !system_active;

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
        }

        Timer::after(Duration::from_millis(100)).await;
    }
}
