//! OCR wbudowany w Windows (Windows.Media.Ocr), z językiem polskim, gdy jest zainstalowany.

use serde::Serialize;
use windows::core::HSTRING;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapDecoder, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};

#[derive(Serialize, Debug)]
pub struct OcrLine {
    pub text: String,
    /// Prostokąt słowa w pikselach obrazu (od lewego górnego rogu).
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OcrResult {
    pub language: String,
    pub lines: Vec<OcrLine>,
}

fn engine(lang: &str) -> windows::core::Result<OcrEngine> {
    let language = Language::CreateLanguage(&HSTRING::from(lang))?;
    if OcrEngine::IsLanguageSupported(&language)? {
        return OcrEngine::TryCreateFromLanguage(&language);
    }
    OcrEngine::TryCreateFromUserProfileLanguages()
}

pub fn recognize(image: &[u8], lang: &str) -> Result<OcrResult, String> {
    // Wątek roboczy tokio nie ma zainicjowanego WinRT; błąd „już zainicjowano” ignorujemy.
    unsafe {
        let _ = windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED);
    }
    let run = || -> windows::core::Result<OcrResult> {
        let stream = InMemoryRandomAccessStream::new()?;
        let writer = DataWriter::CreateDataWriter(&stream)?;
        writer.WriteBytes(image)?;
        writer.StoreAsync()?.join()?;
        writer.FlushAsync()?.join()?;
        writer.DetachStream()?;
        stream.Seek(0)?;
        let decoder = BitmapDecoder::CreateAsync(&stream)?.join()?;
        let bitmap = decoder.GetSoftwareBitmapAsync()?.join()?;
        let bitmap = SoftwareBitmap::Convert(&bitmap, BitmapPixelFormat::Bgra8)?;
        let engine = engine(lang)?;
        let result = engine.RecognizeAsync(&bitmap)?.join()?;
        let mut lines = Vec::new();
        // Słowo po słowie: niewidoczny tekst lepiej pokrywa się z obrazem niż cała linia.
        for line in result.Lines()? {
            for word in line.Words()? {
                let r = word.BoundingRect()?;
                lines.push(OcrLine { text: word.Text()?.to_string(), x: r.X, y: r.Y, w: r.Width, h: r.Height });
            }
        }
        Ok(OcrResult { language: engine.RecognizerLanguage()?.LanguageTag()?.to_string(), lines })
    };
    run().map_err(|e| format!("OCR nie powiódł się: {e}"))
}
