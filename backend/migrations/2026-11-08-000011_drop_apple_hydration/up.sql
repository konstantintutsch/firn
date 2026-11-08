-- Apple Health's water is no longer imported. This removes what an earlier import
-- saved: the values that are the same as the ones in its documents.
DELETE FROM daily_metrics AS metric
    USING garmin_documents AS document
    WHERE document.kind = 'apple_health'
        AND document.user_id = metric.user_id
        AND document.period = metric.day::text
        AND metric.metric = 'hydration_l'
        AND (document.data -> 'metrics' ->> 'hydration_l')::double precision = metric.value;
UPDATE garmin_documents
    SET data = jsonb_set(data, '{metrics}', (data -> 'metrics') - 'hydration_l')
    WHERE kind = 'apple_health' AND data -> 'metrics' ? 'hydration_l';
