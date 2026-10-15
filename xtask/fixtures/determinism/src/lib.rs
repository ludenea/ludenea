//! Uses every item the determinism lints of the simulation crates forbid,
//! so a test can check that each one is reported.

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, RandomState};
use std::time::{Instant, SystemTime};

pub fn f32_methods(x: f32) {
    let _ = x.mul_add(x, x);
    let _ = x.sin();
    let _ = x.cos();
    let _ = x.tan();
    let _ = x.asin();
    let _ = x.acos();
    let _ = x.atan();
    let _ = x.sinh();
    let _ = x.cosh();
    let _ = x.tanh();
    let _ = x.asinh();
    let _ = x.acosh();
    let _ = x.atanh();
    let _ = x.exp();
    let _ = x.exp2();
    let _ = x.exp_m1();
    let _ = x.ln();
    let _ = x.ln_1p();
    let _ = x.log2();
    let _ = x.log10();
    let _ = x.cbrt();
    let _ = x.sin_cos();
    let _ = x.atan2(x);
    let _ = x.hypot(x);
    let _ = x.log(x);
    let _ = x.powf(x);
    let _ = x.min(x);
    let _ = x.max(x);
    let _ = x.powi(2);
}

pub fn f64_methods(x: f64) {
    let _ = x.mul_add(x, x);
    let _ = x.sin();
    let _ = x.cos();
    let _ = x.tan();
    let _ = x.asin();
    let _ = x.acos();
    let _ = x.atan();
    let _ = x.sinh();
    let _ = x.cosh();
    let _ = x.tanh();
    let _ = x.asinh();
    let _ = x.acosh();
    let _ = x.atanh();
    let _ = x.exp();
    let _ = x.exp2();
    let _ = x.exp_m1();
    let _ = x.ln();
    let _ = x.ln_1p();
    let _ = x.log2();
    let _ = x.log10();
    let _ = x.cbrt();
    let _ = x.sin_cos();
    let _ = x.atan2(x);
    let _ = x.hypot(x);
    let _ = x.log(x);
    let _ = x.powf(x);
    let _ = x.min(x);
    let _ = x.max(x);
    let _ = x.powi(2);
}
pub fn types(
    _: HashMap<u8, u8>,
    _: HashSet<u8>,
    _: RandomState,
    _: DefaultHasher,
    _: Instant,
    _: SystemTime,
) {
}

pub fn threads() {
    std::thread::spawn(|| {});
    std::thread::scope(|_| {});
}
