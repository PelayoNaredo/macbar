//! Compartir archivos con el panel de Windows (desde ahí, «Enlace Móvil» los manda al teléfono;
//! también salen Uso compartido cercano, correo, etc.).

use super::*;
use windows::ApplicationModel::DataTransfer::{DataRequestedEventArgs, DataTransferManager};
use windows::Foundation::TypedEventHandler;
use windows::Storage::{IStorageItem, StorageFile};
use windows::Win32::UI::Shell::IDataTransferManagerInterop;
use windows::core::{HSTRING, Interface};

thread_local! {
    /// Manejador registrado en la ventana (se cambia en cada uso: si no, se acumularían)
    static TOKEN: std::cell::Cell<Option<i64>> = const { std::cell::Cell::new(None) };
}

/// Abre el panel de compartir de Windows con estos archivos. Hay que llamarlo desde el hilo de la ventana.
pub fn files(hwnd: HWND, paths: &[String]) -> windows::core::Result<()> {
    // Los archivos se resuelven antes de abrir el panel: dentro del aviso no se puede esperar
    let items: Vec<Option<IStorageItem>> = paths.iter()
        .filter_map(|p| StorageFile::GetFileFromPathAsync(&HSTRING::from(p.as_str())).ok()?.join().ok())
        .filter_map(|f| f.cast::<IStorageItem>().ok()).map(Some).collect();
    if items.is_empty() { return Err(windows::core::Error::from_hresult(windows::core::HRESULT(0x80070002u32 as i32))); }
    let n = items.len();
    let title = if n == 1 {
        std::path::Path::new(&paths[0]).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
    } else { i18n::tf("{} archivos", &[&n]) };
    let list = SendList(items);
    let interop = windows::core::factory::<DataTransferManager, IDataTransferManagerInterop>()?;
    let dtm: DataTransferManager = unsafe { interop.GetForWindow(hwnd)? };
    if let Some(t) = TOKEN.with(|c| c.take()) { let _ = dtm.RemoveDataRequested(t); }
    let token = dtm.DataRequested(&TypedEventHandler::<DataTransferManager, DataRequestedEventArgs>::new(move |_, args| {
        let Some(args) = args.as_ref() else { return Ok(()) };
        let data = args.Request()?.Data()?;
        data.Properties()?.SetTitle(&HSTRING::from(title.as_str()))?;
        let l = &list; // la lista entera (no solo su campo): es ella la que es Send
        let it: windows_collections::IIterable<IStorageItem> = l.0.clone().into();
        data.SetStorageItemsReadOnly(&it)?;
        Ok(())
    }))?;
    TOKEN.with(|c| c.set(Some(token)));
    unsafe { interop.ShowShareUIForWindow(hwnd) }
}

/// Los StorageFile son ágiles (valen desde cualquier hilo); el aviso los pide `Send`.
struct SendList(Vec<Option<IStorageItem>>);
unsafe impl Send for SendList {}
