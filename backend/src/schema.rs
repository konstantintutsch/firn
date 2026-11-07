// @generated automatically by Diesel CLI.

diesel::table! {
    activities (id) {
        id -> Int8,
        name -> Text,
        activity_type -> Text,
        started_at -> Timestamptz,
        distance_m -> Nullable<Float8>,
        duration_s -> Nullable<Float8>,
        summary -> Jsonb,
        track_synced_at -> Nullable<Timestamptz>,
        original_zip -> Nullable<Bytea>,
        description -> Nullable<Text>,
        location -> Nullable<Text>,
        started_local -> Nullable<Text>,
        search_text -> Nullable<Text>,
    }
}

diesel::table! {
    track_points (activity_id, seq) {
        activity_id -> Int8,
        seq -> Int4,
        lat -> Float8,
        lon -> Float8,
        x -> Nullable<Float8>,
        y -> Nullable<Float8>,
    }
}

diesel::joinable!(track_points -> activities (activity_id));

diesel::allow_tables_to_appear_in_same_query!(activities, track_points,);
