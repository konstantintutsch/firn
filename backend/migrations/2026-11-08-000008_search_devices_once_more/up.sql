-- Only the watch and its sensors are kept now, each product once. The files are
-- searched again for the devices.
UPDATE activities SET devices_checked_at = NULL;
