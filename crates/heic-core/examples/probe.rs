use libheif_rs::{HeifContext, LibHeif};
fn main() {
    let _lib = LibHeif::new();
    for p in std::env::args().skip(1) {
        let ctx = HeifContext::read_from_file(&p).unwrap();
        let h = ctx.primary_image_handle().unwrap();
        print!("{p}: {}x{} alpha={} luma_bits={} thumbs={} top={} ",
            h.width(), h.height(), h.has_alpha_channel(), h.luma_bits_per_pixel(),
            h.number_of_thumbnails(), ctx.image_ids().len());
        print!("icc={:?} ", h.color_profile_raw().map(|c| c.data.len()));
        if let Some(n) = h.color_profile_nclx() {
            print!("nclx prim={:?} trc={:?} mc={:?} full={} ", n.color_primaries(), n.transfer_characteristics(), n.matrix_coefficients(), n.full_range_flag());
        }
        for m in h.all_metadata() { print!("meta[{:?} '{}' {}] ", m.item_type, m.content_type, m.raw_data.len()); }
        println!();
    }
}
