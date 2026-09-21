fn main() {
    println!(
        "{} {} — remote device access skeleton (transports: {:?})",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        remote_device_sync::transports(),
    );
}
