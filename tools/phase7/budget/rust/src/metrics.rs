//! Native process high-water mark sampled before observation materialization.
//! This includes startup and preparation; it is not retained-only/compiler-only RSS.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn peak_rss_bytes() -> Result<u64, String> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: getrusage writes a complete rusage to a valid, suitably aligned
    // pointer on success. No uninitialized field is read on failure.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // SAFETY: successful getrusage initialized the entire rusage above.
    let usage = unsafe { usage.assume_init() };
    let value = u64::try_from(usage.ru_maxrss).map_err(|_| "negative process peak RSS")?;
    #[cfg(target_os = "linux")]
    let value = value.checked_mul(1024).ok_or("process peak RSS overflow")?;
    Ok(value)
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn peak_rss_bytes() -> Result<u64, String> {
    Err("process peak RSS sampling requires macOS or Linux".into())
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    #[test]
    fn current_process_high_water_mark_is_available_in_bytes() {
        assert!(super::peak_rss_bytes().unwrap() > 0);
    }
}
