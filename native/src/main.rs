fn main() -> std::io::Result<()> {
    for monitor in mousex_native::monitors()? {
        println!(
            "{}: ({}, {})-({}, {}) primary={}",
            monitor.name,
            monitor.bounds.left,
            monitor.bounds.top,
            monitor.bounds.right,
            monitor.bounds.bottom,
            monitor.primary
        );
    }
    Ok(())
}
