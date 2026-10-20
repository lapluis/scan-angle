//! Windows' built-in TIFF codecs cover palette and fax layouts that the Rust
//! TIFF decoder does not yet support. No external DLL installation is needed.
use anyhow::{Result, bail};
use image::{DynamicImage, RgbImage};
use std::{os::windows::ffi::OsStrExt, path::Path};
use windows::{
    Win32::{
        Foundation::{GENERIC_READ, RPC_E_CHANGED_MODE},
        Graphics::Imaging::*,
        System::Com::*,
    },
    core::PCWSTR,
};
pub fn decode_page(path: &Path, page: usize, allow_large: bool) -> Result<DynamicImage> {
    struct Apartment(bool);
    impl Drop for Apartment {
        fn drop(&mut self) {
            if self.0 {
                unsafe {
                    CoUninitialize();
                }
            }
        }
    }
    // COM objects remain on this thread and are dropped before the apartment.
    unsafe {
        let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
        if hr.is_err() && hr != RPC_E_CHANGED_MODE {
            hr.ok()?;
        }
        let _apartment = Apartment(hr.is_ok());
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let decoder = factory.CreateDecoderFromFilename(
            PCWSTR(path.as_ptr()),
            None,
            GENERIC_READ,
            WICDecodeMetadataCacheOnDemand,
        )?;
        let frame = decoder.GetFrame(u32::try_from(page - 1)?)?;
        let (mut width, mut height) = (0, 0);
        frame.GetSize(&mut width, &mut height)?;
        if !allow_large && u64::from(width) * u64::from(height) > crate::imaging::MAX_PIXELS {
            bail!("图片超过像素保护上限");
        }
        let stride = width
            .checked_mul(3)
            .ok_or_else(|| anyhow::anyhow!("图片尺寸溢出"))?;
        let size = (stride as usize)
            .checked_mul(height as usize)
            .ok_or_else(|| anyhow::anyhow!("图片尺寸溢出"))?;
        let converter = factory.CreateFormatConverter()?;
        converter.Initialize(
            &frame,
            &GUID_WICPixelFormat24bppRGB,
            WICBitmapDitherTypeNone,
            None,
            0.,
            WICBitmapPaletteTypeCustom,
        )?;
        let mut data = Vec::new();
        data.try_reserve_exact(size)?;
        data.resize(size, 0);
        converter.CopyPixels(std::ptr::null(), stride, &mut data)?;
        Ok(DynamicImage::ImageRgb8(
            RgbImage::from_raw(width, height, data)
                .ok_or_else(|| anyhow::anyhow!("图片数据长度不匹配"))?,
        ))
    }
}
