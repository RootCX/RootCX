CREATE TABLE IF NOT EXISTS rootcx_system.file_channels (
    installation_id UUID NOT NULL REFERENCES rootcx_system.app_installations(id) ON DELETE CASCADE,
    channel TEXT NOT NULL,
    PRIMARY KEY (installation_id, channel)
);
CREATE TABLE IF NOT EXISTS rootcx_system.file_publications (
    installation_id UUID NOT NULL,
    channel TEXT NOT NULL,
    key TEXT NOT NULL,
    file_id UUID NOT NULL REFERENCES rootcx_system.files(id) ON DELETE RESTRICT,
    media_type TEXT NOT NULL,
    PRIMARY KEY (installation_id, channel, key),
    FOREIGN KEY (installation_id, channel) REFERENCES rootcx_system.file_channels ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS file_publications_file_id ON rootcx_system.file_publications(file_id);

CREATE OR REPLACE FUNCTION rootcx_system.withdraw_installation_file_channels()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM rootcx_system.file_channels WHERE installation_id = OLD.id;
    RETURN NEW;
END;
$$;
DROP TRIGGER IF EXISTS withdraw_file_channels ON rootcx_system.app_installations;
CREATE TRIGGER withdraw_file_channels AFTER UPDATE OF active ON rootcx_system.app_installations
FOR EACH ROW WHEN (OLD.active AND NOT NEW.active)
EXECUTE FUNCTION rootcx_system.withdraw_installation_file_channels();
