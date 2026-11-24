-- Set the cas club's base to Tobalaba (SCTB)
UPDATE clubs SET aerodrome_icao = 'SCTB', latitude = -33.456, longitude = -70.548 
WHERE slug = 'cas' AND (aerodrome_icao IS NULL OR aerodrome_icao = '' OR aerodrome_icao = 'SCTB');

-- Also set demo club (admin@demo.test) to have SCTB as base if it exists
UPDATE clubs SET aerodrome_icao = 'SCTB', latitude = -33.456, longitude = -70.548 
WHERE contact_email = 'admin@demo.test' AND (aerodrome_icao IS NULL OR aerodrome_icao = '' OR aerodrome_icao = 'SCTB');
