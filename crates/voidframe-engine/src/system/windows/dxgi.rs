//! Primary GPU adapter name via DXGI -- simpler and more standard for a
//! one-time "what GPU is this" string than a first WMI/COM `IWbemLocator`
//! integration would be (spec `docs/superpowers/specs/2026-09-04-thermal-
//! cooldown-hardware-info-design.md` originally suggested WMI; DXGI reads
//! the identical adapter description with far less COM boilerplate).

use crate::error::{Error, Result};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};

pub fn primary_adapter_description() -> Result<String> {
    // SAFETY: `CreateDXGIFactory1` is a standard, documented DXGI entry
    // point with no preconditions beyond linking against dxgi.dll (present
    // on every Windows install with a display driver). The returned
    // `IDXGIFactory1` COM interface manages its own lifetime via `windows`
    // crate's `Result<T>`/`Drop` wrapping -- no manual `Release` needed.
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }
        .map_err(|e| Error::msg(format!("CreateDXGIFactory1 failed: {e}")))?;
    // SAFETY: `factory` was just successfully created above, so `EnumAdapters`
    // is called on a valid, live COM object. Index 0 is DXGI's own documented
    // convention for "the adapter the OS considers primary" on a
    // single-GPU system; a multi-GPU system still returns *a* real adapter
    // at index 0, just not guaranteed to be the one CS2 renders with --
    // acceptable for a one-time informational display string, not used for
    // anything that depends on it being the *active* render adapter.
    let adapter = unsafe { factory.EnumAdapters(0) }
        .map_err(|e| Error::msg(format!("IDXGIFactory1::EnumAdapters(0) failed: {e}")))?;
    // SAFETY: `adapter` was just successfully returned above, so
    // `GetDesc` is called on a valid, live COM object. Unlike the classic
    // Win32 out-parameter shape, this `windows` crate version's binding
    // (0.62) returns `DXGI_ADAPTER_DESC` by value inside its own `Result`
    // rather than writing through a caller-supplied pointer.
    let desc = unsafe { adapter.GetDesc() }
        .map_err(|e| Error::msg(format!("IDXGIAdapter::GetDesc failed: {e}")))?;
    let len = desc
        .Description
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(desc.Description.len());
    Ok(String::from_utf16_lossy(&desc.Description[..len]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_adapter_description_is_a_real_non_empty_string() {
        let name = primary_adapter_description().unwrap();
        assert!(!name.trim().is_empty());
        assert!(
            !name.contains('\u{0}'),
            "description should be trimmed of embedded NULs: {name:?}"
        );
    }
}
