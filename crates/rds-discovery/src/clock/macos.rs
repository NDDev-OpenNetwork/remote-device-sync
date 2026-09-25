// Darwin CLOCK_MONOTONIC includes sleep, unlike Linux CLOCK_MONOTONIC.
// https://github.com/apple-oss-distributions/Libc/blob/main/gen/clock_gettime.3
pub(super) const CLOCK: rustix::time::ClockId = rustix::time::ClockId::Monotonic;

#[allow(unsafe_code)]
pub(super) fn boot_id() -> Result<[u8; 16], String> {
    let mut bytes = [0u8; 64];
    let mut len = bytes.len();
    // SAFETY: the name is NUL-terminated; the writable buffer and length
    // out-parameter are live for the call. A null new value makes this read-only.
    let result = unsafe {
        libc::sysctlbyname(
            c"kern.bootsessionuuid".as_ptr(),
            bytes.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let bytes = bytes.get(..len).ok_or("oversized OS boot identifier")?;
    super::parse_boot(bytes)
}
