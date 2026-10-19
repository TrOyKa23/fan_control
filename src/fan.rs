/// Линейная зависимость PWM от температуры:
/// - ниже 23.0°C => минимальный PWM вентилятора
/// - 23.0°C..26.0°C => линейный рост PWM
/// - выше 26.0°C => максимальный PWM
/// Чувствительность: 0.1°C
pub fn calculate_pwm_compare(temp_deci_c: i16) -> u16 {
    const TOP: u32 = 5999;
    const TEMP_MIN: i16 = 240; // 23.0°C
    const TEMP_MAX: i16 = 260; // 26.0°C
    const PWM_MIN: u32 = 0; // ~10% duty cycle, минимально допустимый PWM для раскрутки

    if temp_deci_c <= TEMP_MIN {
        PWM_MIN as u16
    } else if temp_deci_c >= TEMP_MAX {
        TOP as u16
    } else {
        let temp_range = (TEMP_MAX - TEMP_MIN) as u32; // 30 = 3.0°C = 30 * 0.1°C
        let pwm_range = TOP - PWM_MIN;
        let temp_above_min = (temp_deci_c - TEMP_MIN) as u32; // шаг 1 = 0.1°C

        // Линейная интерполяция, округление к ближайшему значению
        let pwm = PWM_MIN + ((temp_above_min * pwm_range) + (temp_range / 2)) / temp_range;
        pwm as u16
    }
}
