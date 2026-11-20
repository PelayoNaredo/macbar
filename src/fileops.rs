//! Acciones sobre los archivos de la bandeja (como DynaDrop o Droppy): comprimir en ZIP, convertir
//! imágenes a PNG o JPG y copiar el texto de una imagen con el OCR de Windows. Todo en un hilo aparte;
//! el resultado entra en la bandeja y la isla lo avisa.

use super::*;

/// Formatos de imagen que se pueden convertir o leer con OCR.
pub fn is_image(path: &str) -> bool {
    let ext = std::path::Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "bmp" | "gif" | "tif" | "tiff" | "webp" | "heic" | "jxr" | "ico")
}

/// Ruta libre junto a `dir` con ese nombre: «x.png», «x (2).png», «x (3).png»…
pub fn unique(dir: &std::path::Path, stem: &str, ext: &str) -> std::path::PathBuf {
    let mut p = dir.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while p.exists() { p = dir.join(format!("{stem} ({n}).{ext}")); n += 1; }
    p
}

fn done(ok: bool, what: u8, out: Option<String>) {
    if let Some(o) = out.filter(|_| ok) { live::shelf_add(vec![o]); }
    live::show(live::Hud::FileOp(what, ok), 2600);
}

/// Comprime los archivos en un ZIP junto al primero (con el tar.exe que trae Windows).
pub fn zip(paths: Vec<String>) {
    std::thread::spawn(move || {
        let Some(first) = paths.first().map(std::path::PathBuf::from) else { return };
        let dir = first.parent().map(|d| d.to_path_buf()).unwrap_or_default();
        let stem = if paths.len() == 1 { first.file_stem().and_then(|s| s.to_str()).unwrap_or("Archivo").to_string() } else { i18n::t("Archivos").to_string() };
        let out = unique(&dir, &stem, "zip");
        let ok = zip_to(&paths, &out);
        done(ok, 0, Some(out.to_string_lossy().to_string()));
    });
}

fn zip_to(paths: &[String], out: &std::path::Path) -> bool {
    // Un -C por archivo: dentro del ZIP cada uno va con su nombre, sin la ruta entera
    let mut cmd = std::process::Command::new("tar.exe");
    cmd.arg("-a").arg("-c").arg("-f").arg(out);
    for p in paths {
        let pb = std::path::Path::new(p);
        if let (Some(d), Some(n)) = (pb.parent(), pb.file_name()) { cmd.arg("-C").arg(d).arg(n); }
    }
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000 /* CREATE_NO_WINDOW */).status().is_ok_and(|s| s.success()) && out.exists()
}

/// Convierte una imagen a PNG (`png` = true) o a JPG, al lado de la original.
pub fn convert(path: String, png: bool) {
    std::thread::spawn(move || {
        unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
        let src = std::path::PathBuf::from(&path);
        let dir = src.parent().map(|d| d.to_path_buf()).unwrap_or_default();
        let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("imagen").to_string();
        let out = unique(&dir, &stem, if png { "png" } else { "jpg" });
        let ok = encode(&path, &out.to_string_lossy(), png).is_some();
        if !ok { let _ = std::fs::remove_file(&out); }
        done(ok, if png { 1 } else { 2 }, Some(out.to_string_lossy().to_string()));
    });
}

fn encode(src: &str, out: &str, png: bool) -> Option<()> {
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let dec = wic.CreateDecoderFromFilename(&HSTRING::from(src), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand).ok()?;
        let frame = dec.GetFrame(0).ok()?;
        let (fmt, container) = if png { (GUID_WICPixelFormat32bppBGRA, GUID_ContainerFormatPng) } else { (GUID_WICPixelFormat24bppBGR, GUID_ContainerFormatJpeg) };
        let conv = wic.CreateFormatConverter().ok()?;
        conv.Initialize(&frame, &fmt, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        conv.GetSize(&mut w, &mut h).ok()?;
        let stream = wic.CreateStream().ok()?;
        stream.InitializeFromFilename(&HSTRING::from(out), GENERIC_WRITE.0).ok()?;
        let enc = wic.CreateEncoder(&container, std::ptr::null()).ok()?;
        enc.Initialize(&stream, WICBitmapEncoderNoCache).ok()?;
        let mut fe: Option<IWICBitmapFrameEncode> = None;
        let mut props = None;
        enc.CreateNewFrame(&mut fe, &mut props).ok()?;
        let fe = fe?;
        fe.Initialize(props.as_ref()).ok()?;
        fe.SetSize(w, h).ok()?;
        let mut pf = fmt;
        fe.SetPixelFormat(&mut pf).ok()?;
        fe.WriteSource(&conv, std::ptr::null()).ok()?;
        fe.Commit().ok()?;
        enc.Commit().ok()?;
        Some(())
    }
}

/// Lee el texto de una imagen con el OCR de Windows (idiomas del perfil) y lo copia.
pub fn ocr(path: String) {
    std::thread::spawn(move || {
        unsafe { let _ = RoInitialize(RO_INIT_MULTITHREADED); }
        match ocr_text(&path) {
            Some(t) => {
                deskw::copy_text(&t);
                *live::CLIP.lock().unwrap() = trunc(&t.replace('\n', " "), 26);
                live::show(live::Hud::FileOp(3, true), 2600);
            }
            None => live::show(live::Hud::FileOp(3, false), 2600),
        }
    });
}

/// Texto de una imagen con el OCR de Windows (idiomas del perfil), línea a línea.
fn ocr_text(path: &str) -> Option<String> {
    use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat};
    let file = windows::Storage::StorageFile::GetFileFromPathAsync(&HSTRING::from(path)).ok()?.join().ok()?;
    let stream = file.OpenAsync(windows::Storage::FileAccessMode::Read).ok()?.join().ok()?;
    let dec = BitmapDecoder::CreateAsync(&stream).ok()?.join().ok()?;
    let bmp = dec.GetSoftwareBitmapConvertedAsync(BitmapPixelFormat::Bgra8, BitmapAlphaMode::Premultiplied).ok()?.join().ok()?;
    let eng = windows::Media::Ocr::OcrEngine::TryCreateFromUserProfileLanguages().ok()?;
    // Línea a línea (el texto de corrido lo junta todo con espacios)
    let res = eng.RecognizeAsync(&bmp).ok()?.join().ok()?;
    let lines = res.Lines().ok()?;
    let mut out = Vec::new();
    for i in 0..lines.Size().unwrap_or(0) { if let Ok(l) = lines.GetAt(i) { out.push(l.Text().map(|t| t.to_string()).unwrap_or_default()); } }
    let t = out.join("\n");
    (!t.trim().is_empty()).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imagenes_y_nombres() {
        assert!(is_image("C:\\x\\Foto.JPG"));
        assert!(is_image("a.webp"));
        assert!(!is_image("informe.pdf"));
        assert!(!is_image("carpeta"));
        let dir = std::env::temp_dir().join(format!("macbar_fileops_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let a = unique(&dir, "x", "png");
        assert_eq!(a.file_name().unwrap(), "x.png");
        std::fs::write(&a, b"1").unwrap();
        assert_eq!(unique(&dir, "x", "png").file_name().unwrap(), "x (2).png");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Las tres acciones de verdad, sobre copias en una carpeta temporal
    #[test]
    fn convertir_comprimir_y_leer() {
        unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
        let dir = std::env::temp_dir().join(format!("macbar_ops_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs").join("macbar-search.png");
        let png = dir.join("captura.png");
        std::fs::copy(&src, &png).unwrap();
        let jpg = dir.join("captura.jpg");
        assert!(encode(&png.to_string_lossy(), &jpg.to_string_lossy(), false).is_some());
        assert!(std::fs::metadata(&jpg).unwrap().len() > 1000);
        let back = dir.join("vuelta.png");
        assert!(encode(&jpg.to_string_lossy(), &back.to_string_lossy(), true).is_some());
        let z = dir.join("todo.zip");
        assert!(zip_to(&[png.to_string_lossy().to_string(), jpg.to_string_lossy().to_string()], &z));
        assert!(std::fs::metadata(&z).unwrap().len() > 1000);
        // La captura del buscador tiene texto: el OCR tiene que encontrar algo
        let t = ocr_text(&png.to_string_lossy());
        assert!(t.as_ref().is_some_and(|t| t.len() > 5), "OCR: {t:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
