//! The renderer page's frame fixture (`editor/e2e/fixtures/renderer/`): `frame.postcard` is
//! `frame.json` in the control protocol's postcard encoding, which the page pushes both ways
//! to check that they draw alike. After a change to `frame.json` or to the encoding of a
//! `RenderFrame`, write it again with
//! `LUDENEA_WRITE_FIXTURES=1 cargo test -p ludenea-render --test web_fixture`.

use std::fs;
use std::path::PathBuf;

use ludenea_draw::frame::RenderFrame;

#[test]
fn the_page_frame_bytes_are_its_json_in_postcard() {
    let folder =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../editor/e2e/fixtures/renderer");
    let json = fs::read_to_string(folder.join("frame.json")).unwrap();
    let frame: RenderFrame = serde_json::from_str(&json).unwrap();
    // `ludenea_control::protocol::encode_frame`'s encoding.
    let expected = postcard::to_allocvec(&frame).unwrap();
    let path = folder.join("frame.postcard");
    if std::env::var_os("LUDENEA_WRITE_FIXTURES").is_some() {
        fs::write(&path, &expected).unwrap();
    }
    let found = fs::read(&path).unwrap_or_default();
    assert!(
        found == expected,
        "{} is not frame.json in postcard: write it again with \
         LUDENEA_WRITE_FIXTURES=1 cargo test -p ludenea-render --test web_fixture",
        path.display()
    );
}
