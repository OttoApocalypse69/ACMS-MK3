//! Images on the system clipboard, so sprites can be copied to and from other programs.

use std::borrow::Cow;

use acms_core::image::Image;

/// Puts an image on the system clipboard. A failure is logged and otherwise ignored: the
/// app's own clipboard still has the image.
pub fn write_image(image: &Image, log: &mut Vec<String>) {
    let data = arboard::ImageData {
        width: image.width() as usize,
        height: image.height() as usize,
        bytes: Cow::Owned(image.to_rgba_bytes()),
    };
    let result = arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_image(data));
    if let Err(err) = result {
        log.push(format!("Could not copy to the system clipboard: {err}"));
    }
}

/// The image on the system clipboard, if it holds one.
pub fn read_image() -> Option<Image> {
    let data = arboard::Clipboard::new().ok()?.get_image().ok()?;
    let (width, height) = (
        u32::try_from(data.width).ok()?,
        u32::try_from(data.height).ok()?,
    );
    Image::from_rgba_bytes(width, height, &data.bytes)
}
