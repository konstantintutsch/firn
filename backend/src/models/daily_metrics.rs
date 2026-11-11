use uuid::Uuid;

/// A VO2 max from Apple Health, which belongs to a workout if there is one for it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Vo2Reading {
    pub value: f64,
    pub activity_id: Option<Uuid>,
}
