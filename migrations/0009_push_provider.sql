-- Which push service a device's token is for: Apple's (`apns`, also when unset) or Firebase's.
ALTER TABLE devices ADD COLUMN push_provider TEXT CHECK (push_provider IN ('apns', 'fcm'));
