//! Arrow IPC streams for frames.

use std::io::Cursor;

use arrow_array::RecordBatch;
use arrow_ipc::reader::StreamReader;
use arrow_ipc::writer::StreamWriter;
use arrow_schema::{ArrowError, Schema};

/// Encodes one batch as an Arrow IPC stream.
///
/// # Errors
///
/// Returns an error if Arrow cannot encode the batch.
pub fn encode_batch(batch: &RecordBatch) -> Result<Vec<u8>, ArrowError> {
    let mut out = Vec::new();
    let mut writer = StreamWriter::try_new(&mut out, &batch.schema())?;
    writer.write(batch)?;
    writer.finish()?;
    drop(writer);
    Ok(out)
}

/// Encodes a schema alone as an Arrow IPC stream with no batches.
///
/// # Errors
///
/// Returns an error if Arrow cannot encode the schema.
pub fn encode_schema(schema: &Schema) -> Result<Vec<u8>, ArrowError> {
    let mut out = Vec::new();
    let mut writer = StreamWriter::try_new(&mut out, schema)?;
    writer.finish()?;
    drop(writer);
    Ok(out)
}

/// Decodes an Arrow IPC stream into its schema and batches. A stream with a schema
/// and no batches is an empty result, not an error.
///
/// # Errors
///
/// Returns an error if the bytes are not a valid stream.
pub fn decode_stream(
    bytes: &[u8],
) -> Result<(arrow_schema::SchemaRef, Vec<RecordBatch>), ArrowError> {
    let reader = StreamReader::try_new(Cursor::new(bytes), None)?;
    let schema = reader.schema();
    let batches = reader.collect::<Result<Vec<_>, _>>()?;
    Ok((schema, batches))
}

/// Decodes every batch of an Arrow IPC stream.
///
/// # Errors
///
/// Returns an error if the bytes are not a valid stream.
pub fn decode_batches(bytes: &[u8]) -> Result<Vec<RecordBatch>, ArrowError> {
    StreamReader::try_new(Cursor::new(bytes), None)?.collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow_array::{Int64Array, StringArray};

    use super::*;

    #[test]
    fn round_trips() {
        let batch = RecordBatch::try_from_iter([
            ("a", Arc::new(Int64Array::from(vec![1, 2])) as _),
            ("b", Arc::new(StringArray::from(vec![Some("x"), None])) as _),
        ])
        .unwrap();
        let decoded = decode_batches(&encode_batch(&batch).unwrap()).unwrap();
        assert_eq!(decoded, vec![batch]);
    }
}
