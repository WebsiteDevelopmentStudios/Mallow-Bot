use image::{imageops::{overlay, resize_to_fill, FilterType}, DynamicImage, ImageFormat, Rgba, RgbaImage};
use imageproc::drawing::draw_text_mut;
use ab_glyph::{FontRef, PxScale};
use std::io::Cursor;
use tracing::warn;

const DEFAULT_BACKGROUND: &str = "https://raw.githubusercontent.com/WebsiteDevelopmentStudios/Mallow-Bot/main/dashboard/Welcome.png";
const MAX_IMAGE_BYTES: usize = 2 * 1024 * 1024;

pub async fn render_member_card(
    client: &reqwest::Client,
    avatar_url: &str,
    background_url: Option<&str>,
    display_name: &str,
    member_number: u64,
    leaving: bool,
) -> Option<Vec<u8>> {
    let avatar_bytes = fetch(client, avatar_url).await?;
    let background = match background_url.filter(|url| url.starts_with("https://") || url.starts_with("http://")) {
        Some(url) => fetch(client, url).await.or_else(|| None),
        None => None,
    };
    let background = match background {
        Some(bytes) => Some(bytes),
        None => fetch(client, DEFAULT_BACKGROUND).await,
    }?;
    compose(background, avatar_bytes, display_name, member_number, leaving)
}

async fn fetch(client: &reqwest::Client, url: &str) -> Option<Vec<u8>> {
    let response = client.get(url).header("User-Agent", "Mallow-Bot/1.0").send().await.ok()?;
    if !response.status().is_success() { return None; }
    let bytes = response.bytes().await.ok()?;
    if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES { return None; }
    Some(bytes.to_vec())
}

fn compose(background: Vec<u8>, avatar: Vec<u8>, display_name: &str, member_number: u64, leaving: bool) -> Option<Vec<u8>> {
    let background = image::load_from_memory(&background).ok()?.resize_exact(700, 400, FilterType::Lanczos3).to_rgba8();
    let avatar = image::load_from_memory(&avatar).ok()?;
    let mut canvas: RgbaImage = background;
    let mut face = resize_to_fill(&avatar, 112, 112, FilterType::Lanczos3).to_rgba8();
    let center = 56.0_f32;
    for (x, y, pixel) in face.enumerate_pixels_mut() {
        let dx = x as f32 + 0.5 - center;
        let dy = y as f32 + 0.5 - center;
        if dx * dx + dy * dy > center * center { pixel.0[3] = 0; }
    }
    overlay(&mut canvas, &face, 35, 245);

    let font_bytes = std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf").ok()?;
    let font = FontRef::try_from_slice(&font_bytes).ok()?;
    let safe_name: String = display_name.chars().filter(|c| !c.is_control()).take(64).collect();
    draw_text_mut(&mut canvas, Rgba([255, 255, 255, 255]), 174, 272, PxScale::from(30.0), &font, &safe_name);
    let detail = format!("This user {} #{} on the server", if leaving { "was" } else { "is" }, member_number.max(1));
    draw_text_mut(&mut canvas, Rgba([224, 218, 242, 255]), 174, 319, PxScale::from(18.0), &font, &detail);

    let mut output = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(canvas).write_to(&mut output, ImageFormat::Png).ok()?;
    Some(output.into_inner())
}
