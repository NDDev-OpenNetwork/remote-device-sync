// CLOCK_BOOTTIME advances during suspend (Linux clock_gettime(2)).
pub(super) const CLOCK: rustix::time::ClockId = rustix::time::ClockId::Boottime;

pub(super) fn boot_id() -> Result<[u8; 16], String> {
    use std::io::Read;
    let file = std::fs::File::open("/proc/sys/kernel/random/boot_id").map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(65)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    super::parse_boot(&bytes)
}
