use ab_glyph::{FontArc, PxScale};
use image::{imageops, DynamicImage, GenericImageView, ImageFormat, Rgba, RgbaImage};
use imageproc::drawing::draw_text_mut;
use std::{io::Cursor, net::IpAddr, time::Duration};
use tracing::warn;

const DEFAULT_BACKGROUND: &str = "https://raw.githubusercontent.com/WebsiteDevelopmentStudios/Mallow-Bot/main/dashboard/Welcome.png";

async fn download(url: &str, max_bytes: usize) -> Option<Vec<u8>> {
    let parsed = url::Url::parse(url).ok()?;
    if parsed.scheme() != "https" { return None; }
    let host = parsed.host_str()?.to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") || host.ends_with(".internal") { return None; }
    if let Ok(ip) = host.parse::<IpAddr>() {
        let blocked = match ip {
            IpAddr::V4(ip) => ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || ip.is_multicast(),
            IpAddr::V6(ip) => ip.is_loopback() || ip.is_unspecified() || ip.is_unique_local() || ip.is_unicast_link_local() || ip.is_multicast(),
        };
        if blocked { return None; }
    }
    let client = reqwest::Client::builder().timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none()).build().ok()?;
    let response = client.get(url).send().await.ok()?;
    if !response.status().is_success() { return None; }
    if response.content_length().unwrap_or(0) > max_bytes as u64 { return None; }
    let bytes = response.bytes().await.ok()?;
    if bytes.len() > max_bytes { return None; }
    Some(bytes.to_vec())
}

fn font(paths: &[&str]) -> Option<FontArc> {
    paths.iter().find_map(|path| std::fs::read(path).ok().and_then(|bytes| FontArc::try_from_vec(bytes).ok()))
}

fn fit(source: DynamicImage, width: u32, height: u32) -> RgbaImage {
    let (sw, sh) = source.dimensions();
    let scale = (width as f32 / sw as f32).max(height as f32 / sh as f32);
    let rw = (sw as f32 * scale).ceil() as u32;
    let rh = (sh as f32 * scale).ceil() as u32;
    let resized = source.resize_exact(rw, rh, imageops::FilterType::Lanczos3).to_rgba8();
    let x = (rw - width) / 2;
    let y = (rh - height) / 2;
    imageops::crop_imm(&resized, x, y, width, height).to_image()
}

pub async fn render(background_url: &str, avatar_url: &str, username: &str, member_number: u64, leaving: bool) -> Option<Vec<u8>> {
    let avatar_bytes = download(avatar_url, 2 * 1024 * 1024).await?;
    let background_bytes = match download(background_url, 5 * 1024 * 1024).await {
        Some(bytes) => bytes,
        None => download(DEFAULT_BACKGROUND, 5 * 1024 * 1024).await?,
    };
    let background = image::load_from_memory(&background_bytes).ok()?;
    let avatar = image::load_from_memory(&avatar_bytes).ok()?;
    let mut canvas = fit(background, 700, 400);
    let mut face = imageops::resize(&avatar, 112, 112, imageops::FilterType::Lanczos3);
    for y in 0..112 {
        for x in 0..112 {
            let dx = x as f32 - 55.5;
            let dy = y as f32 - 55.5;
            if dx * dx + dy * dy > 55.5 * 55.5 { face.get_pixel_mut(x, y).0[3] = 0; }
        }
    }
    for y in 248..378 {
        for x in 22..678 {
            let pixel = canvas.get_pixel_mut(x, y);
            let alpha = 220u16;
            let source = [13u16, 12u16, 20u16];
            for (index, color) in source.iter().enumerate() {
                pixel.0[index] = ((*color * alpha + pixel.0[index] as u16 * (255 - alpha)) / 255) as u8;
            }
            pixel.0[3] = 255;
        }
    }
    imageops::overlay(&mut canvas, &face, 42, 257);
    let regular = font(&[
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation2/LiberationSans-Regular.ttf",
    ]);
    let bold = font(&[
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        "/usr/share/fonts/truetype/liberation2/LiberationSans-Bold.ttf",
    ]).or_else(|| regular.clone());
    let (Some(regular), Some(bold)) = (regular, bold) else {
        warn!("No supported font was found for member-card rendering.");
        return None;
    };
    let safe_name: String = username.replace('\n', " ").chars().take(34).collect();
    draw_text_mut(&mut canvas, Rgba([255,255,255,255]), 174, 271, PxScale::from(28.0), &bold, &safe_name);
    let detail = format!("This user {} #{} on the server", if leaving { "was" } else { "is" }, member_number.max(1));
    draw_text_mut(&mut canvas, Rgba([224,218,242,255]), 174, 320, PxScale::from(18.0), &regular, &detail);
    let mut output = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(canvas).write_to(&mut output, ImageFormat::Png).ok()?;
    Some(output.into_inner())
}
