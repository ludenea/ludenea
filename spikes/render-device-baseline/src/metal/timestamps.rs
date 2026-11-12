//! The device's timestamps around a render pass: a counter sample buffer of
//! two samples taken at the boundaries of the pass's stages, where the device
//! offers its timestamp counter set there, and the device's clock read beside
//! the process's clock to convert the device's ticks. Throwaway, never
//! product code.

use std::ffi::c_void;
use std::ptr;
use std::time::{Duration, Instant};

use super::gpu::Gpu;
use super::objc::{Id, Owned, error_text, new_object, present, text};
use super::values;

/// The device's clock and the process's clock, read together.
pub struct Clock {
    taken: Instant,
    device_ticks: u64,
}

impl Clock {
    /// Nothing where the device does not answer `sampleTimestamps:gpuTimestamp:`.
    pub fn read(device: Id) -> Option<Clock> {
        let (mut processor_ticks, mut device_ticks) = (0_u64, 0_u64);
        send!((); device, "sampleTimestamps:gpuTimestamp:", *mut u64: &mut processor_ticks, *mut u64: &mut device_ticks).ok()?;
        Some(Clock { taken: Instant::now(), device_ticks })
    }

    /// The nanoseconds one tick of the device's clock lasts, measured from
    /// this reading to now, with the time the measure spans. Nothing where
    /// the device's clock did not advance.
    pub fn nanoseconds_per_tick(&self, device: Id) -> Option<(f64, Duration)> {
        let now = Clock::read(device)?;
        let ticks = now.device_ticks.checked_sub(self.device_ticks).filter(|ticks| *ticks > 0)?;
        let elapsed = now.taken.duration_since(self.taken);
        Some((elapsed.as_nanos() as f64 / ticks as f64, elapsed))
    }
}

/// The names of the counter sets the device lists.
pub fn counter_set_names(device: Id) -> Result<Vec<String>, String> {
    Ok(counter_sets(device)?.into_iter().map(|(name, _)| name).collect())
}

fn counter_sets(device: Id) -> Result<Vec<(String, Id)>, String> {
    let sets = send!(Id; device, "counterSets")?;
    if sets.is_null() {
        return Ok(Vec::new());
    }
    let mut named = Vec::new();
    for index in 0..send!(usize; sets, "count")? {
        let set = present(send!(Id; sets, "objectAtIndex:", usize: index)?, "objectAtIndex: of counterSets")?;
        named.push((text(send!(Id; set, "name")?), set));
    }
    Ok(named)
}

/// Whether the device samples counters at each point it knows, in the order
/// of `values::COUNTER_SAMPLING_POINT_NAMES`; nothing where it does not answer.
pub fn sampling_points(device: Id) -> Option<Vec<bool>> {
    (0..values::COUNTER_SAMPLING_POINT_NAMES.len()).map(|point| send!(u8; device, "supportsCounterSampling:", usize: point).ok().map(|answer| answer != 0)).collect()
}

/// A counter sample buffer of two timestamps, written at the start of a
/// pass's vertex stage and at the end of its fragment stage.
pub struct StageTimestamps {
    buffer: Owned,
}

impl StageTimestamps {
    /// The error says what the device lacks for it.
    pub fn create(gpu: &Gpu) -> Result<StageTimestamps, String> {
        let device = gpu.device.id();
        let sets = counter_sets(device)?;
        let Some((_, timestamp_set)) = sets.iter().find(|(name, _)| *name == gpu.timestamp_set) else {
            return Err(format!("counterSets has no set named {}", gpu.timestamp_set));
        };
        if send!(u8; device, "supportsCounterSampling:", usize: values::COUNTER_SAMPLING_POINT_AT_STAGE_BOUNDARY)? == 0 {
            return Err("supportsCounterSampling: refuses the stage boundary".to_string());
        }
        let descriptor = new_object("MTLCounterSampleBufferDescriptor")?;
        send!((); descriptor.id(), "setCounterSet:", Id: *timestamp_set)?;
        send!((); descriptor.id(), "setStorageMode:", usize: values::STORAGE_MODE_SHARED)?;
        send!((); descriptor.id(), "setSampleCount:", usize: 2)?;
        let mut error: Id = ptr::null_mut();
        let buffer = send!(Id; device, "newCounterSampleBufferWithDescriptor:error:", Id: descriptor.id(), *mut Id: &mut error)?;
        if buffer.is_null() {
            return Err(format!("newCounterSampleBufferWithDescriptor:error: {}", error_text(error)));
        }
        Ok(StageTimestamps { buffer: Owned::adopt(buffer, "newCounterSampleBufferWithDescriptor:error:")? })
    }

    /// Makes a pass write the two samples: the first when its vertex stage
    /// starts, the second when its fragment stage ends.
    pub fn attach(&self, pass: Id) -> Result<(), String> {
        let attachments = present(send!(Id; pass, "sampleBufferAttachments")?, "sampleBufferAttachments of the pass descriptor")?;
        let attachment = present(send!(Id; attachments, "objectAtIndexedSubscript:", usize: 0)?, "objectAtIndexedSubscript: of the pass's sample buffer attachments")?;
        send!((); attachment, "setSampleBuffer:", Id: self.buffer.id())?;
        send!((); attachment, "setStartOfVertexSampleIndex:", usize: 0)?;
        send!((); attachment, "setEndOfVertexSampleIndex:", usize: values::COUNTER_DONT_SAMPLE)?;
        send!((); attachment, "setStartOfFragmentSampleIndex:", usize: values::COUNTER_DONT_SAMPLE)?;
        send!((); attachment, "setEndOfFragmentSampleIndex:", usize: 1)
    }

    /// The time between the two samples once the pass has completed, the
    /// device's ticks converted with its clock where it gives one and taken
    /// as nanoseconds otherwise. Nothing where a sample is missing.
    pub fn elapsed_nanoseconds(&self, gpu: &Gpu) -> Option<u64> {
        let data = send!(Id; self.buffer.id(), "resolveCounterRange:", values::Range: values::Range { location: 0, length: 2 }).ok()?;
        let bytes = send!(*const c_void; data, "bytes").ok()?;
        let length = send!(usize; data, "length").ok()?;
        if bytes.is_null() || length < 2 * size_of::<u64>() {
            return None;
        }
        // SAFETY: the data holds at least two `MTLCounterResultTimestamp`,
        // each one 64-bit unsigned integer, and lives as long as the pool.
        let (start, end) = unsafe { (ptr::read_unaligned(bytes as *const u64), ptr::read_unaligned((bytes as *const u64).add(1))) };
        if start == values::COUNTER_ERROR_VALUE || end == values::COUNTER_ERROR_VALUE || end < start {
            return None;
        }
        let nanoseconds_per_tick = gpu.clock.as_ref().and_then(|clock| clock.nanoseconds_per_tick(gpu.device.id())).map_or(1.0, |(nanoseconds, _)| nanoseconds);
        Some(((end - start) as f64 * nanoseconds_per_tick).round() as u64)
    }
}
