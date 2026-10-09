//! Main-thread AppKit smoke probe; uses an isolated named pasteboard only.
fn main() -> Result<(), rds_desktop::DesktopError> {
    let cases = rds_desktop::render::native_clipboard_probe()?;
    println!("native clipboard cases passed: {cases}");
    Ok(())
}
