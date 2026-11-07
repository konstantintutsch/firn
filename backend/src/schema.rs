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
        user_id -> Nullable<Int8>,
        details -> Nullable<Jsonb>,
    }
}

diesel::table! {
    daily_metrics (user_id, metric, day) {
        user_id -> Int8,
        day -> Date,
        metric -> Text,
        value -> Float8,
    }
}

diesel::table! {
    garmin_documents (user_id, kind, period) {
        user_id -> Int8,
        kind -> Text,
        period -> Text,
        data -> Nullable<Jsonb>,
        fetched_at -> Timestamptz,
    }
}

diesel::table! {
    sessions (token_hash) {
        token_hash -> Bytea,
        user_id -> Int8,
        created_at -> Timestamptz,
        expires_at -> Timestamptz,
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

diesel::table! {
    users (id) {
        id -> Int8,
        oidc_issuer -> Text,
        oidc_subject -> Text,
        email -> Nullable<Text>,
        name -> Nullable<Text>,
        garmin_api_key -> Nullable<Bytea>,
        created_at -> Timestamptz,
    }
}

diesel::joinable!(activities -> users (user_id));
diesel::joinable!(daily_metrics -> users (user_id));
diesel::joinable!(garmin_documents -> users (user_id));
diesel::joinable!(sessions -> users (user_id));
diesel::joinable!(track_points -> activities (activity_id));

diesel::allow_tables_to_appear_in_same_query!(
    activities,
    daily_metrics,
    garmin_documents,
    sessions,
    track_points,
    users,
);
