fn main() {
    if let Err(error) = slint_build::compile("src/app.slint") {
        eprintln!("Slint UI compilation failed: {error}");
        std::process::exit(1);
    }
}
