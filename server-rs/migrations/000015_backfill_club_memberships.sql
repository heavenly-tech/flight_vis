-- Prod (CAS): pilots had home club + role but club_memberships was empty (0 rows).

INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
SELECT replace(gen_random_uuid()::text, '-', ''), p.id, p.club_id, p.role, p.status, p.created_at
FROM pilots p
WHERE p.status IN ('approved', 'pending')
  AND p.role IN ('admin', 'pilot')
ON CONFLICT (pilot_id, club_id) DO NOTHING;