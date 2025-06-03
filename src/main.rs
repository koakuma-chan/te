const MIN_TEXT_LEN: usize = 256;

const MAX_TEXT_LEN: usize = 32_768;

fn extract_pdf(input: &[u8]) -> Result<String, String> {
    use lopdf::{Document, Object};

    fn extract_ocr(buf: &mut String, input: &[u8]) -> Result<(), String> {
        use std::{
            io::Write,
            process::{Command, Stdio},
        };

        use tempfile::NamedTempFile;

        if input.is_empty() {
            return Ok(());
        }

        let Some(kind) = infer::get(input) else {
            return Ok(());
        };
        match kind.mime_type() {
            "image/png" | "image/jpeg" | "image/tiff" | "image/gif" | "image/webp" => (),

            _ => return Ok(()),
        }

        let mut file = NamedTempFile::with_suffix(&format!(".{}", kind.extension()))
            //
            .map_err(|e| format!("failed to create a temporary file: {e:?}"))?;
        file.write_all(input)
            //
            .map_err(|e| format!("failed to write image data to file: {e:?}"))?;

        let output = Command::new("tesseract")
            //
            .arg(file.path())
            //
            .arg("stdout")
            //
            .stdin(Stdio::null())
            //
            .stdout(Stdio::piped())
            //
            .stderr(Stdio::piped())
            //
            .output()
            //
            .map_err(|e| format!("failed to run ocr: {e:?}"))?;

        if output.status.success() {
            for chunk in output.stdout.utf8_chunks() {
                buf.push_str(chunk.valid());
                if buf.len() > MAX_TEXT_LEN {
                    return Err(format!("invalid text length: {}", buf.len()));
                }
            }

            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);

            Err(format!("ocr failed: {stderr}"))
        }
    }

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

        for (object_id, _) in document.objects.iter() {
            if let Ok(object) = document.get_object(*object_id) {
                if let Object::Stream(stream) = object {
                    if let Ok(subtype) = stream.dict.get(b"Subtype") {
                        if let Object::Name(name) = subtype {
                            if name == b"Image" {
                                extract_ocr(&mut buf, &stream.content)?;
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
        buf.push('\n');

        let mut stack = Vec::<std::slice::Iter<'_, ParagraphChild>>::new();

        stack.push(paragraph.iter());

        while let Some(iter) = stack.last_mut() {
            if let Some(child) = iter.next() {
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
            } else {
                stack.pop();
            }
        }

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
        return Err("invalid input length".to_string());
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
