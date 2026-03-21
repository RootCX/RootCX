DO $$
BEGIN
  IF EXISTS (
    SELECT 1 FROM information_schema.columns
    WHERE table_schema = 'rootcx_system' AND table_name = 'users' AND column_name = 'username'
  ) THEN
    ALTER TABLE rootcx_system.users ADD COLUMN IF NOT EXISTS email TEXT UNIQUE;
    UPDATE rootcx_system.users SET email = username || '@localhost' WHERE email IS NULL;
    ALTER TABLE rootcx_system.users ALTER COLUMN email SET NOT NULL;
    ALTER TABLE rootcx_system.users DROP COLUMN username;
  END IF;
END $$;
