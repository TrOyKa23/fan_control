use embassy_rp::gpio::{Flex, Pull};

const CYCLES_PER_MICROS: u32 = 75;

#[inline(always)]
fn delay_us(us: u32) {
    cortex_m::asm::delay(us * CYCLES_PER_MICROS);
}

pub struct OneWireBus<'a> {
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

    pub fn search_rom(&mut self, roms: &mut [[u8; 8]; 6]) -> usize {
        let mut count = 0;
        let mut last_discrepancy = 0i8;
        let mut last_device_flag = false;
        let mut last_rom = [0u8; 8];

        while !last_device_flag && count < 6 {
            if !self.reset() {
                break;
            }

            self.write_byte(0xF0);
            let mut last_zero = 0i8;
            let mut rom_byte_index = 0usize;
            let mut rom_byte_mask = 1u8;
            let mut current_rom = [0u8; 8];
            let mut search_result = true;

            for bit_number in 1..=64i8 {
                let id_bit = self.read_bit();
                let cmp_id_bit = self.read_bit();

                if id_bit && cmp_id_bit {
                    search_result = false;
                    break;
                }

                let search_direction = if !id_bit && !cmp_id_bit {
                    let dir = if bit_number == last_discrepancy {
                        true
                    } else if bit_number > last_discrepancy {
                        false
                    } else {
                        (last_rom[rom_byte_index] & rom_byte_mask) != 0
                    };

                    if !dir {
                        last_zero = bit_number;
                    }
                    dir
                } else {
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
