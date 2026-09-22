-- Keep user IDs, SSO identities, sessions and role assignments intact.
ALTER TABLE rootcx_system.users DROP COLUMN IF EXISTS password_hash;
