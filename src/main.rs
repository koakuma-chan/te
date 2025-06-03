const MIN_TEXT_LEN: usize = 256;

const MAX_TEXT_LEN: usize = 32_768;

fn extract_pdf(input: &[u8]) -> Result<String, String> {
    use lopdf::{Document, Object};

    use tesseract_plumbing::TessBaseApi;

    use leptonica_plumbing::Pix;

    let document = Document::load_mem(input)
        //
        .map_err(|e| format!("failed to read input as pdf: {e:?}"))?;

    let mut buf = document
        //
        .extract_text(
            //
            &document
                .get_pages()
                //
                .into_keys()
                //
                .collect::<Vec<_>>(),
        )
        //
        .map_err(|e| format!("failed to extract text: {e:?}"))?;

    let effective_len = buf.trim().len();
    if effective_len < MIN_TEXT_LEN {
        buf.clear();

        let mut tesseract = TessBaseApi::create();
        if let Err(e) = tesseract.init_4(
            //
            Some(c"/usr/share/tesseract-ocr/5/tessdata"),
            //
            Some(c"eng"),
            //
            tesseract_sys::TessOcrEngineMode_OEM_LSTM_ONLY,
        ) {
            return Err(format!("failed to initialize ocr: {e:?}"));
        }

        for (object_id, _) in document.objects.iter() {
            if let Ok(object) = document.get_object(*object_id) {
                if let Object::Stream(stream) = object {
                    if let Ok(subtype) = stream.dict.get(b"Subtype") {
                        if let Object::Name(name) = subtype {
                            if name == b"Image" {
                                let data = &stream.content;
                                if data.is_empty() {
                                    continue;
                                }

                                let pix = match Pix::read_mem(&data) {
                                    Ok(pix) => pix,

                                    Err(_) => {
                                        eprintln!("failed to read image data");

                                        continue;
                                    }
                                };

                                tesseract.set_image_2(&pix);

                                match tesseract.get_utf8_text() {
                                    Ok(text) => match text.as_ref().to_str() {
                                        Ok(text_str) => {
                                            buf.push_str(text_str);
                                            if buf.len() > MAX_TEXT_LEN {
                                                return Err(format!(
                                                    "invalid text length: {}",
                                                    buf.len()
                                                ));
                                            }

                                            buf.push('\n');
                                        }
                                        Err(e) => {
                                            eprintln!("failed to extract text: {e:?}");
                                        }
                                    },
                                    Err(e) => {
                                        eprintln!("failed to extract text: {e:?}");
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let effective_len = buf.trim().len();
    if effective_len < MIN_TEXT_LEN || effective_len > MAX_TEXT_LEN {
        Err(format!("invalid text length: {effective_len}"))
    } else {
        Ok(buf)
    }
}

fn extract_docx(input: &[u8]) -> Result<String, String> {
    use docx_rs::{
        DocumentChild, ParagraphChild, RunChild, TableCellContent, TableChild, TableRowChild,
    };

    fn extract_paragraph(buf: &mut String, paragraph: &[ParagraphChild]) -> Result<(), String> {
        for child in paragraph {
            if let ParagraphChild::Run(run) = child {
                for child in &run.children {
                    match child {
                        RunChild::Text(text) => {
                            buf.push_str(&text.text);
                            if buf.len() > MAX_TEXT_LEN {
                                return Err(format!("invalid text length: {}", buf.len()));
                            }
                        }

                        RunChild::Break(_) => buf.push('\n'),

                        RunChild::Tab(_) => buf.push('\t'),

                        _ => (),
                    }
                }
            }
        }

        buf.push('\n');

        Ok(())
    }

    let docx = docx_rs::read_docx(input)
        //
        .map_err(|e| format!("failed to read input as docx: {e:?}"))?;

    let mut buf = String::with_capacity(4096);

    for node in &docx.document.children {
        match node {
            DocumentChild::Paragraph(paragraph) => {
                extract_paragraph(&mut buf, &paragraph.children)?;
            }

            DocumentChild::Table(table) => {
                for TableChild::TableRow(row) in &table.rows {
                    for TableRowChild::TableCell(cell) in &row.cells {
                        for child in &cell.children {
                            match child {
                                TableCellContent::Paragraph(paragraph) => {
                                    extract_paragraph(&mut buf, &paragraph.children)?;
                                }

                                _ => (),
                            }
                        }

                        buf.push('\t');
                    }
                }

                buf.push('\n');
            }

            _ => (),
        }
    }

    let effective_len = buf.trim().len();
    if effective_len < MIN_TEXT_LEN {
        return Err(format!("invalid text length: {effective_len}"));
    }

    Ok(buf)
}

fn set_self_batch() {
    let _ = scheduler::set_self_policy(scheduler::Policy::Batch, 0);
}

fn dispatch(input: &[u8]) -> Result<String, String> {
    if input.len() < 256 || input.len() > 5 * 1024 * 1024 {
        return Err(format!("invalid input length: {} bytes", input.len()));
    }

    set_self_batch();

    let Some(kind) = infer::get(input) else {
        return Err("unknown file kind".to_string());
    };

    match kind.mime_type() {
        //
        "application/pdf" => {
            //
            extract_pdf(input)
        }
        //
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            //
            extract_docx(input)
        }
        //
        other => {
            //
            return Err(format!("unsupported file type: {other}"));
        }
    }
}

use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

fn main() {
    use std::io::{self, Read};

    use serde::Serialize;

    #[derive(Serialize)]
    #[serde(tag = "type")]
    enum ExtractionResult {
        //
        Text { text: String },
        //
        Error { error: String },
    }

    let mut input = Vec::new();

    let result = match io::stdin()
        //
        .read_to_end(&mut input)
        //
        .map_err(|e| format!("failed to read input: {e}"))
        //
        .and_then(|_| dispatch(&input))
    {
        //
        Ok(text) => ExtractionResult::Text { text },
        //
        Err(error) => ExtractionResult::Error { error },
    };

    match serde_json::to_string(&result) {
        //
        Ok(output) => println!("{output}"),
        //
        Err(error) => eprintln!("{error}"),
    }
}
