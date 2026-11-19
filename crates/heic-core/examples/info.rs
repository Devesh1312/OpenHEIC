fn main() {
    for p in std::env::args().skip(1) {
        let b = std::fs::read(&p).unwrap();
        println!("{p}: {:#?}", heic_core::read_photo_info(&b));
    }
}
