-- Random IDs, so that a route's link says nothing about how many others exist and
-- an exported route keeps its ID when it is imported into another Runmap.
ALTER TABLE routes ALTER COLUMN id DROP DEFAULT;
ALTER TABLE routes ALTER COLUMN id TYPE UUID USING gen_random_uuid();
ALTER TABLE routes ALTER COLUMN id SET DEFAULT gen_random_uuid();
DROP SEQUENCE routes_id_seq;

-- A few points of the route to draw a small picture of it, so that the list does
-- not load all points. One [latitude, longitude] each.
ALTER TABLE routes ADD COLUMN preview JSONB NOT NULL DEFAULT '[]';
UPDATE routes SET preview = (
    SELECT jsonb_agg(jsonb_build_array(point -> 0, point -> 1) ORDER BY position)
    FROM jsonb_array_elements(points) WITH ORDINALITY AS points_list (point, position)
    WHERE (position - 1) % GREATEST(1, jsonb_array_length(routes.points) / 64) = 0
);
