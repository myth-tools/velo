use std::sync::Arc;

use arrow::array::types::Float32Type;
use arrow::array::{Array, FixedSizeListArray, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use futures_util::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use uuid::Uuid;

use super::error::MemoryError;

const TABLE_NAME: &str = "episodic_memory";

pub fn episodic_memory_schema(dimension: i32) -> Schema {
    Schema::new(vec![
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                dimension,
            ),
            true,
        ),
        Field::new("id", DataType::Utf8, false),
        Field::new("text_content", DataType::Utf8, true),
        Field::new("metadata", DataType::Utf8, true),
    ])
}

#[derive(Clone)]
pub struct VectorStore {
    uri: String,
    dimension: usize,
    table: Option<lancedb::Table>,
}

impl VectorStore {
    pub fn new(uri: String, dimension: usize) -> Self {
        Self {
            uri,
            dimension,
            table: None,
        }
    }

    pub async fn open_or_create(&mut self) -> Result<(), MemoryError> {
        let conn = lancedb::connect(&self.uri).execute().await?;

        let table_names = conn.table_names().execute().await?;
        let has_table = table_names.iter().any(|n| n == TABLE_NAME);

        if !has_table {
            let schema = Arc::new(episodic_memory_schema(self.dimension as i32));
            let table = conn
                .create_empty_table(TABLE_NAME, schema)
                .execute()
                .await?;
            self.table = Some(table);
            tracing::info!("Created episodic_memory table in LanceDB at {}", self.uri);
        } else {
            let table = conn.open_table(TABLE_NAME).execute().await?;
            self.table = Some(table);
            tracing::info!("Opened existing episodic_memory table in LanceDB");
        }

        Ok(())
    }

    pub async fn insert(
        &self,
        id: Uuid,
        vector: Vec<f32>,
        text_content: &str,
        metadata: &str,
    ) -> Result<(), MemoryError> {
        let table = self
            .table
            .as_ref()
            .ok_or_else(|| MemoryError::NotInitialized("LanceDB table not opened".into()))?;

        let dim = self.dimension;
        let mut padded = vector;
        if padded.len() < dim {
            padded.resize(dim, 0.0);
        } else if padded.len() > dim {
            padded.truncate(dim);
        }

        let id_array = StringArray::from(vec![id.to_string()]);
        let text_array = StringArray::from(vec![text_content]);
        let meta_array = StringArray::from(vec![metadata]);

        let vector_values: Vec<Option<f32>> = padded.into_iter().map(Some).collect();
        let vector_array = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
            [Some(vector_values)],
            self.dimension as i32,
        );

        let schema = Arc::new(episodic_memory_schema(self.dimension as i32));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(vector_array),
                Arc::new(id_array),
                Arc::new(text_array),
                Arc::new(meta_array),
            ],
        )?;

        table.add(vec![batch]).execute().await?;
        Ok(())
    }

    pub async fn batch_insert(
        &self,
        records: Vec<(Uuid, Vec<f32>, String, String)>,
    ) -> Result<(), MemoryError> {
        let table = self
            .table
            .as_ref()
            .ok_or_else(|| MemoryError::NotInitialized("LanceDB table not opened".into()))?;

        if records.is_empty() {
            return Ok(());
        }

        let dim = self.dimension;
        let mut ids = Vec::with_capacity(records.len());
        let mut texts = Vec::with_capacity(records.len());
        let mut metas = Vec::with_capacity(records.len());
        let mut vectors: Vec<Vec<Option<f32>>> = Vec::with_capacity(records.len());

        for (id, mut vec, text, meta) in records {
            if vec.len() < dim {
                vec.resize(dim, 0.0);
            } else if vec.len() > dim {
                vec.truncate(dim);
            }
            ids.push(id.to_string());
            texts.push(text);
            metas.push(meta);
            vectors.push(vec.into_iter().map(Some).collect());
        }

        let id_array = StringArray::from(ids);
        let text_array = StringArray::from(texts);
        let meta_array = StringArray::from(metas);

        let vector_array = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
            vectors.into_iter().map(Some),
            self.dimension as i32,
        );

        let schema = Arc::new(episodic_memory_schema(self.dimension as i32));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(vector_array),
                Arc::new(id_array),
                Arc::new(text_array),
                Arc::new(meta_array),
            ],
        )?;

        table.add(vec![batch]).execute().await?;
        Ok(())
    }

    pub async fn search(
        &self,
        query_vector: &[f32],
        limit: usize,
    ) -> Result<Vec<SemanticMemoryRecord>, MemoryError> {
        let table = self
            .table
            .as_ref()
            .ok_or_else(|| MemoryError::NotInitialized("LanceDB table not opened".into()))?;

        let stream = table
            .query()
            .limit(limit)
            .nearest_to(query_vector)?
            .execute()
            .await?;

        let batches: Vec<RecordBatch> = stream.try_collect().await?;
        let mut results = Vec::new();

        for batch in batches {
            let id_array = batch
                .column_by_name("id")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>());
            let text_array = batch
                .column_by_name("text_content")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>());
            let meta_array = batch
                .column_by_name("metadata")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>());

            if let Some(ids) = id_array {
                for i in 0..ids.len() {
                    let id_str = ids.value(i);
                    let id = match Uuid::parse_str(id_str) {
                        Ok(u) => u,
                        Err(e) => {
                            tracing::warn!("Invalid UUID in vector store: {e}");
                            continue;
                        }
                    };
                    let text = text_array
                        .map(|t| t.value(i).to_string())
                        .unwrap_or_default();
                    let metadata = meta_array
                        .map(|m| m.value(i).to_string())
                        .unwrap_or_default();

                    results.push(SemanticMemoryRecord {
                        id,
                        text_content: text,
                        metadata,
                    });
                }
            }
        }

        Ok(results)
    }

    pub async fn delete_by_id(&self, id: Uuid) -> Result<(), MemoryError> {
        let table = self
            .table
            .as_ref()
            .ok_or_else(|| MemoryError::NotInitialized("LanceDB table not opened".into()))?;

        let predicate = format!("id = '{id}'");
        table.delete(&predicate).await?;
        Ok(())
    }

    pub async fn close(&self) -> Result<(), MemoryError> {
        tracing::info!("Vector store closed");
        Ok(())
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SemanticMemoryRecord {
    pub id: Uuid,
    pub text_content: String,
    pub metadata: String,
}
