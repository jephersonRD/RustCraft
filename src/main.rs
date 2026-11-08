fn main() {
    if let Err(error) = rustcraft::cli::run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
