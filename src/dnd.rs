//! Arrastrar archivos con OLE: soltarlos en la isla o en la tarjeta (se guardan en la bandeja, y al
//! entrar en la isla la tarjeta se abre en su pestaña) y sacarlos de la bandeja hacia otra app.

use super::*;
use std::cell::Cell;
use windows::core::{implement, Ref, BOOL, HRESULT};
use windows::Win32::Foundation::{DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, POINTL};
use windows::Win32::System::Com::{IBindCtx, IDataObject, DVASPECT_CONTENT, FORMATETC, STGMEDIUM, TYMED_HGLOBAL};
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;

fn hdrop_format() -> FORMATETC {
    FORMATETC { cfFormat: 15 /* CF_HDROP */, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 }
}

pub(crate) fn has_files(d: &IDataObject) -> bool { unsafe { d.QueryGetData(&hdrop_format()) == windows::Win32::Foundation::S_OK } }

/// Rutas de los archivos que trae el objeto arrastrado.
pub(crate) fn paths_of(d: &IDataObject) -> Vec<String> {
    let mut out = Vec::new();
    unsafe {
        let Ok(mut med) = d.GetData(&hdrop_format()) else { return out };
        let hd = HDROP(med.u.hGlobal.0);
        let n = DragQueryFileW(hd, u32::MAX, None);
        for i in 0..n {
            let mut buf = [0u16; 1024];
            let l = DragQueryFileW(hd, i, Some(&mut buf));
            out.push(String::from_utf16_lossy(&buf[..l as usize]));
        }
        ReleaseStgMedium(&mut med as *mut STGMEDIUM);
    }
    out
}

/// Destino de soltar: la barra (abre la tarjeta al entrar con archivos en la isla) o la propia tarjeta.
#[implement(IDropTarget)]
struct Target { card: bool, files: Cell<bool> }

impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(&self, data: Ref<IDataObject>, _keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        let files = data.as_ref().is_some_and(has_files);
        self.files.set(files);
        if files {
            if self.card { card::drop_hover(true); } else if over_island(pt) { card::open_for_drop(); }
        }
        unsafe { *effect = if files { DROPEFFECT_COPY } else { DROPEFFECT_NONE }; }
        Ok(())
    }
    fn DragOver(&self, _keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        let files = self.files.get();
        // En la barra, solo la isla recoge archivos (y al llegar a ella se abre la tarjeta)
        if files && !self.card && over_island(pt) && !card::is_open() { card::open_for_drop(); }
        let ok = files && (self.card || over_island(pt));
        unsafe { *effect = if ok { DROPEFFECT_COPY } else { DROPEFFECT_NONE }; }
        Ok(())
    }
    fn DragLeave(&self) -> windows::core::Result<()> {
        if self.card { card::drop_hover(false); }
        Ok(())
    }
    fn Drop(&self, data: Ref<IDataObject>, _keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        let paths = data.as_ref().map(paths_of).unwrap_or_default();
        let ok = !paths.is_empty() && (self.card || over_island(pt));
        if ok { live::shelf_add(paths); }
        if self.card { card::drop_hover(false); card::dropped(); }
        unsafe { *effect = if ok { DROPEFFECT_COPY } else { DROPEFFECT_NONE }; }
        Ok(())
    }
}

/// ¿El punto (pantalla) cae sobre la isla de la barra (con un margen generoso)?
fn over_island(pt: &POINTL) -> bool {
    let bar = bar_hwnd();
    let s = scale_of(bar);
    let mut br = RECT::default();
    unsafe { let _ = GetWindowRect(bar, &mut br); }
    let (x, y) = ((pt.x - br.left) as f32 / s, (pt.y - br.top) as f32 / s);
    GFX.with(|g| g.try_borrow().ok().and_then(|g| g.as_ref().map(|g| {
        let r = g.isl_rect;
        // Sin nada en la isla, su sitio es el centro de la barra
        let (l, rr) = if r.right - r.left < 20.0 { let c = (r.left + r.right) / 2.0; (c - 90.0, c + 90.0) } else { (r.left - 24.0, r.right + 24.0) };
        x >= l && x <= rr && y >= 0.0 && y <= BAR_H
    }))).unwrap_or(false)
}

/// Registra la barra y la tarjeta como destinos de soltar (OLE: sustituye a WM_DROPFILES).
pub fn register(hwnd: HWND, card: bool) {
    unsafe {
        let _ = OleInitialize(None);
        let t: IDropTarget = Target { card, files: Cell::new(false) }.into();
        let _ = RegisterDragDrop(hwnd, &t);
    }
}

// ---------------------------------------------------------------- arrastrar hacia fuera
#[implement(IDropSource)]
struct Source;

impl IDropSource_Impl for Source_Impl {
    fn QueryContinueDrag(&self, esc: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if esc.as_bool() { return DRAGDROP_S_CANCEL; }
        if keys.0 & MK_LBUTTON.0 == 0 { return DRAGDROP_S_DROP; }
        HRESULT(0)
    }
    fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT { DRAGDROP_S_USEDEFAULTCURSORS }
}

/// Arrastra un archivo de la bandeja hacia otra app (con el objeto de datos del propio explorador).
/// Devuelve el efecto: si se movió, el archivo ya no está donde estaba.
pub fn drag_out(path: &str) -> Option<DROPEFFECT> {
    unsafe {
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        SHParseDisplayName(&HSTRING::from(path), None::<&IBindCtx>, &mut pidl, 0, None).ok()?;
        let arr = SHCreateShellItemArrayFromIDLists(&[pidl as *const ITEMIDLIST]);
        ILFree(Some(pidl));
        let data: IDataObject = arr.ok()?.BindToHandler(None::<&IBindCtx>, &BHID_DataObject).ok()?;
        let src: IDropSource = Source.into();
        let mut eff = DROPEFFECT_NONE;
        let r = DoDragDrop(&data, &src, DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK, &mut eff);
        (r == DRAGDROP_S_DROP).then_some(eff)
    }
}
