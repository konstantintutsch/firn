// @generated automatically by Diesel CLI.

diesel::table! {
    garmin_tokens (id) {
        id -> Bool,
        access_token -> Text,
        refresh_token -> Text,
        client_id -> Text,
        updated_at -> Timestamptz,
    }
}

diesel::table! {
    runs (id) {
        id -> Int8,
        name -> Text,
        activity_type -> Text,
        started_at -> Timestamptz,
        distance_m -> Nullable<Float8>,
        duration_s -> Nullable<Float8>,
        summary -> Jsonb,
        gpx -> Nullable<Text>,
        track_synced_at -> Nullable<Timestamptz>,
    }
}

diesel::table! {
    track_points (run_id, seq) {
        run_id -> Int8,
        seq -> Int4,
        lat -> Float8,
        lon -> Float8,
        x -> Nullable<Float8>,
        y -> Nullable<Float8>,
    }
}

diesel::joinable!(track_points -> runs (run_id));

diesel::allow_tables_to_appear_in_same_query!(garmin_tokens, runs, track_points,);
