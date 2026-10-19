use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};

// Temperatures stored as deci-degrees Celsius (one unit = 0.1 °C)
// Use AtomicI32 for each sensor.
pub static TEMP0: AtomicI32 = AtomicI32::new(0);
pub static TEMP1: AtomicI32 = AtomicI32::new(0);
pub static TEMP2: AtomicI32 = AtomicI32::new(0);
pub static TEMP3: AtomicI32 = AtomicI32::new(0);
pub static TEMP4: AtomicI32 = AtomicI32::new(0);
pub static TEMP5: AtomicI32 = AtomicI32::new(0);

pub static RPM: AtomicU32 = AtomicU32::new(0);
pub static SYSTEM_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn set_temp(index: usize, temp_deci_c: i16) {
    let v = temp_deci_c as i32;
    match index {
        0 => TEMP0.store(v, Ordering::Relaxed),
        1 => TEMP1.store(v, Ordering::Relaxed),
        2 => TEMP2.store(v, Ordering::Relaxed),
        3 => TEMP3.store(v, Ordering::Relaxed),
        4 => TEMP4.store(v, Ordering::Relaxed),
        5 => TEMP5.store(v, Ordering::Relaxed),
        _ => {}
    }
}

pub fn get_temps() -> [i16; 6] {
    [
        TEMP0.load(Ordering::Relaxed) as i16,
        TEMP1.load(Ordering::Relaxed) as i16,
        TEMP2.load(Ordering::Relaxed) as i16,
        TEMP3.load(Ordering::Relaxed) as i16,
        TEMP4.load(Ordering::Relaxed) as i16,
        TEMP5.load(Ordering::Relaxed) as i16,
    ]
}

pub fn set_rpm(r: u32) {
    RPM.store(r, Ordering::Relaxed);
}

pub fn get_rpm() -> u32 {
    RPM.load(Ordering::Relaxed)
}

pub fn set_system_active(v: bool) {
    SYSTEM_ACTIVE.store(v, Ordering::Relaxed);
}

pub fn get_system_active() -> bool {
    SYSTEM_ACTIVE.load(Ordering::Relaxed)
}
