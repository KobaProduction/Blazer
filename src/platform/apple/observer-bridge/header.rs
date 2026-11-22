fn main() -> std::io::Result<()> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or_else(|| std::io::Error::other("expected header output path"))?;
    observer_bridge::generate_header(std::path::Path::new(&path))
}
