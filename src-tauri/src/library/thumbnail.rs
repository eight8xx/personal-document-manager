use std::path::Path;

use super::error::{LibraryError, LibraryResult};

pub const MAX_THUMBNAIL_WIDTH: u32 = 320;
pub const MAX_THUMBNAIL_HEIGHT: u32 = 240;

pub fn generate_png_thumbnail(path: &Path, file_type: &str) -> LibraryResult<Vec<u8>> {
    match file_type {
        "JPG" | "PNG" => platform::image_thumbnail(path),
        "PDF" => platform::pdf_first_page_thumbnail(path),
        _ => Err(LibraryError::Preview(format!(
            "不支持生成 {file_type} 缩略图。"
        ))),
    }
}

fn fit_dimensions(width: u32, height: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (1, 1);
    }

    let width_scale = MAX_THUMBNAIL_WIDTH as f64 / width as f64;
    let height_scale = MAX_THUMBNAIL_HEIGHT as f64 / height as f64;
    let scale = width_scale.min(height_scale).min(1.0);
    (
        ((width as f64 * scale).round() as u32).max(1),
        ((height as f64 * scale).round() as u32).max(1),
    )
}

#[cfg(target_os = "windows")]
mod platform {
    use std::fs;
    use std::path::Path;

    use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
    use windows::Graphics::Imaging::{
        BitmapAlphaMode, BitmapDecoder, BitmapEncoder, BitmapInterpolationMode, BitmapPixelFormat,
        BitmapTransform, ColorManagementMode, ExifOrientationMode,
    };
    use windows::Storage::Streams::{DataReader, DataWriter, InMemoryRandomAccessStream};
    use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

    use super::{fit_dimensions, LibraryError, LibraryResult};

    pub(super) fn image_thumbnail(path: &Path) -> LibraryResult<Vec<u8>> {
        initialize_winrt();
        let bytes = fs::read(path)?;
        let input = stream_from_bytes(&bytes)?;
        let decoder = BitmapDecoder::CreateAsync(&input)
            .and_then(|operation| operation.get())
            .map_err(thumbnail_error)?;
        let (width, height) = fit_dimensions(
            decoder.PixelWidth().map_err(thumbnail_error)?,
            decoder.PixelHeight().map_err(thumbnail_error)?,
        );

        let transform = BitmapTransform::new().map_err(thumbnail_error)?;
        transform
            .SetInterpolationMode(BitmapInterpolationMode::Fant)
            .map_err(thumbnail_error)?;
        transform.SetScaledWidth(width).map_err(thumbnail_error)?;
        transform.SetScaledHeight(height).map_err(thumbnail_error)?;
        let bitmap = decoder
            .GetSoftwareBitmapTransformedAsync(
                BitmapPixelFormat::Bgra8,
                BitmapAlphaMode::Premultiplied,
                &transform,
                ExifOrientationMode::RespectExifOrientation,
                ColorManagementMode::ColorManageToSRgb,
            )
            .and_then(|operation| operation.get())
            .map_err(thumbnail_error)?;

        encode_png(&bitmap)
    }

    pub(super) fn pdf_first_page_thumbnail(path: &Path) -> LibraryResult<Vec<u8>> {
        initialize_winrt();
        let bytes = fs::read(path)?;
        let input = stream_from_bytes(&bytes)?;
        let document = PdfDocument::LoadFromStreamAsync(&input)
            .and_then(|operation| operation.get())
            .map_err(thumbnail_error)?;
        if document.PageCount().map_err(thumbnail_error)? == 0 {
            return Err(LibraryError::Preview(
                "PDF 没有可生成缩略图的页面。".to_string(),
            ));
        }

        let page = document.GetPage(0).map_err(thumbnail_error)?;
        let page_size = page.Size().map_err(thumbnail_error)?;
        let (width, height) = fit_dimensions(
            page_size.Width.round().max(1.0) as u32,
            page_size.Height.round().max(1.0) as u32,
        );
        let options = PdfPageRenderOptions::new().map_err(thumbnail_error)?;
        options
            .SetDestinationWidth(width)
            .map_err(thumbnail_error)?;
        options
            .SetDestinationHeight(height)
            .map_err(thumbnail_error)?;
        options
            .SetBitmapEncoderId(BitmapEncoder::PngEncoderId().map_err(thumbnail_error)?)
            .map_err(thumbnail_error)?;

        let output = InMemoryRandomAccessStream::new().map_err(thumbnail_error)?;
        page.RenderWithOptionsToStreamAsync(&output, &options)
            .and_then(|operation| operation.get())
            .map_err(thumbnail_error)?;
        let thumbnail = read_stream(&output)?;
        let _ = page.Close();
        Ok(thumbnail)
    }

    fn encode_png(bitmap: &windows::Graphics::Imaging::SoftwareBitmap) -> LibraryResult<Vec<u8>> {
        let output = InMemoryRandomAccessStream::new().map_err(thumbnail_error)?;
        let encoder = BitmapEncoder::CreateAsync(
            BitmapEncoder::PngEncoderId().map_err(thumbnail_error)?,
            &output,
        )
        .and_then(|operation| operation.get())
        .map_err(thumbnail_error)?;
        encoder.SetSoftwareBitmap(bitmap).map_err(thumbnail_error)?;
        encoder
            .FlushAsync()
            .and_then(|operation| operation.get())
            .map_err(thumbnail_error)?;
        read_stream(&output)
    }

    fn stream_from_bytes(bytes: &[u8]) -> LibraryResult<InMemoryRandomAccessStream> {
        let stream = InMemoryRandomAccessStream::new().map_err(thumbnail_error)?;
        let writer = DataWriter::CreateDataWriter(&stream).map_err(thumbnail_error)?;
        writer.WriteBytes(bytes).map_err(thumbnail_error)?;
        writer
            .StoreAsync()
            .and_then(|operation| operation.get())
            .map_err(thumbnail_error)?;
        writer
            .FlushAsync()
            .and_then(|operation| operation.get())
            .map_err(thumbnail_error)?;
        writer.DetachStream().map_err(thumbnail_error)?;
        stream.Seek(0).map_err(thumbnail_error)?;
        Ok(stream)
    }

    fn read_stream(stream: &InMemoryRandomAccessStream) -> LibraryResult<Vec<u8>> {
        let length = stream.Size().map_err(thumbnail_error)?;
        let length = u32::try_from(length)
            .map_err(|_| LibraryError::Preview("缩略图数据过大。".to_string()))?;
        stream.Seek(0).map_err(thumbnail_error)?;
        let reader = DataReader::CreateDataReader(stream).map_err(thumbnail_error)?;
        reader
            .LoadAsync(length)
            .and_then(|operation| operation.get())
            .map_err(thumbnail_error)?;
        let mut bytes = vec![0_u8; length as usize];
        reader.ReadBytes(&mut bytes).map_err(thumbnail_error)?;
        let _ = reader.Close();
        Ok(bytes)
    }

    fn initialize_winrt() {
        // RPC_E_CHANGED_MODE means the thread already has a different COM apartment,
        // which is valid for the synchronous WinRT calls used here.
        let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    }

    fn thumbnail_error(error: windows::core::Error) -> LibraryError {
        LibraryError::Preview(format!("无法生成缩略图：{error}"))
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use std::path::Path;

    use super::{LibraryError, LibraryResult};

    pub(super) fn image_thumbnail(_path: &Path) -> LibraryResult<Vec<u8>> {
        Err(LibraryError::Preview(
            "真实缩略图生成当前仅在 Windows 上受支持。".to_string(),
        ))
    }

    pub(super) fn pdf_first_page_thumbnail(_path: &Path) -> LibraryResult<Vec<u8>> {
        Err(LibraryError::Preview(
            "PDF 首页缩略图当前仅在 Windows 上受支持。".to_string(),
        ))
    }
}
