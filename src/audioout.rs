//! Salidas de audio: cuáles hay, cuál es la de por defecto, cambiarla y su volumen (para la tarjeta de
//! la isla). Cambiar la salida por defecto no tiene API pública: se usa la misma interfaz (no documentada,
//! estable desde Windows 7) que usan el propio Windows y herramientas como EarTrumpet o SoundSwitch.

use super::*;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::STGM_READ;

/// Una salida: id del dispositivo, nombre y si es la de por defecto.
#[derive(Clone, PartialEq, Debug)]
pub struct Output { pub id: String, pub name: String, pub default: bool }

/// Icono de cada salida según su nombre (auriculares, pantalla o altavoces).
pub fn glyph(name: &str) -> &'static str {
    let n = name.to_lowercase();
    if ["auricular", "headphone", "headset", "airpods", "buds", "cascos"].iter().any(|k| n.contains(k)) { "\u{E7F6}" }
    else if ["hdmi", "display", "monitor", "pantalla", "tv", "nvidia", "amd high definition"].iter().any(|k| n.contains(k)) { "\u{E7F4}" }
    else { "\u{E7F5}" }
}

pub fn outputs() -> Vec<Output> {
    if sys::demo() {
        return vec![
            Output { id: "a".into(), name: "Altavoces (Realtek Audio)".into(), default: false },
            Output { id: "b".into(), name: "Auriculares (Sony WH-1000XM5)".into(), default: true },
            Output { id: "c".into(), name: "LG ULTRAGEAR (NVIDIA High Definition Audio)".into(), default: false },
        ];
    }
    let mut out = Vec::new();
    unsafe {
        let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return out };
        let def = en.GetDefaultAudioEndpoint(eRender, eConsole).ok().and_then(|d| d.GetId().ok()).map(|p| { let s = p.to_string().unwrap_or_default(); CoTaskMemFree(Some(p.0 as _)); s }).unwrap_or_default();
        let Ok(col) = en.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) else { return out };
        for i in 0..col.GetCount().unwrap_or(0) {
            let Ok(d) = col.Item(i) else { continue };
            let Ok(p) = d.GetId() else { continue };
            let id = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as _));
            let name = d.OpenPropertyStore(STGM_READ).ok().and_then(|ps| ps.GetValue(&PKEY_Device_FriendlyName).ok()).map(|v| v.to_string()).unwrap_or_default();
            out.push(Output { default: id == def, id, name });
        }
    }
    out
}

/// Hace de `id` la salida por defecto (para todo: multimedia, consola y comunicaciones).
pub fn set_default(id: &str) {
    if sys::demo() { return; }
    use windows::core::{Interface, GUID, IUnknown};
    // CLSID_PolicyConfigClient e IID_IPolicyConfig (Windows 7 en adelante)
    const CLSID: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
    const IID: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);
    unsafe {
        let Ok(unk) = CoCreateInstance::<_, IUnknown>(&CLSID, None, CLSCTX_ALL) else { return };
        let mut pc: *mut core::ffi::c_void = std::ptr::null_mut();
        if unk.query(&IID, &mut pc).is_err() || pc.is_null() { return; }
        // SetDefaultEndpoint es el método 11 de la interfaz (tras los 3 de IUnknown): posición 13 de la tabla
        type SetDefault = unsafe extern "system" fn(*mut core::ffi::c_void, PCWSTR, ERole) -> windows::core::HRESULT;
        type Release = unsafe extern "system" fn(*mut core::ffi::c_void) -> u32;
        let vt = *(pc as *const *const usize);
        let set: SetDefault = std::mem::transmute(*vt.add(13));
        let w = HSTRING::from(id);
        for role in [eConsole, eMultimedia, eCommunications] { let _ = set(pc, PCWSTR(w.as_ptr()), role); }
        let rel: Release = std::mem::transmute(*vt.add(2));
        rel(pc);
    }
}

fn endpoint() -> Option<IAudioEndpointVolume> {
    unsafe {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        en.GetDefaultAudioEndpoint(eRender, eConsole).ok()?.Activate(CLSCTX_ALL, None).ok()
    }
}

/// Volumen (0..1) y silencio de la salida por defecto.
pub fn volume() -> (f32, bool) {
    if sys::demo() { return (0.62, false); }
    endpoint().map(|v| unsafe { (v.GetMasterVolumeLevelScalar().unwrap_or(0.0), v.GetMute().map(|b| b.as_bool()).unwrap_or(false)) }).unwrap_or((0.0, false))
}

pub fn set_volume(x: f32) {
    if let Some(v) = endpoint() { unsafe { let _ = v.SetMasterVolumeLevelScalar(x.clamp(0.0, 1.0), std::ptr::null()); if x > 0.0 { let _ = v.SetMute(false, std::ptr::null()); } } }
}

pub fn toggle_mute() {
    if let Some(v) = endpoint() { unsafe { let m = v.GetMute().map(|b| b.as_bool()).unwrap_or(false); let _ = v.SetMute(!m, std::ptr::null()); } }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iconos_de_las_salidas() {
        assert_eq!(glyph("Auriculares (Sony WH-1000XM5)"), "\u{E7F6}");
        assert_eq!(glyph("Headset Earphone (AirPods)"), "\u{E7F6}");
        assert_eq!(glyph("LG ULTRAGEAR (NVIDIA High Definition Audio)"), "\u{E7F4}");
        assert_eq!(glyph("Altavoces (Realtek(R) Audio)"), "\u{E7F5}");
    }
}
