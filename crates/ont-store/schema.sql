PRAGMA application_id = 1330533430;
PRAGMA user_version = 1;
CREATE TABLE objects (
 object_type TEXT COLLATE BINARY NOT NULL,
 instance TEXT COLLATE BINARY NOT NULL,
 account TEXT COLLATE BINARY NOT NULL,
 native_key TEXT COLLATE BINARY NOT NULL,
 scope TEXT NOT NULL, head TEXT NOT NULL,
 PRIMARY KEY(object_type, instance, account, native_key)
) WITHOUT ROWID;
CREATE TABLE observations (
 object_type TEXT COLLATE BINARY NOT NULL,
 instance TEXT COLLATE BINARY NOT NULL,
 account TEXT COLLATE BINARY NOT NULL,
 native_key TEXT COLLATE BINARY NOT NULL,
 revision TEXT COLLATE BINARY NOT NULL,
 ordinal INTEGER NOT NULL,
 payload TEXT NOT NULL,
 PRIMARY KEY(object_type, instance, account, native_key, revision),
 UNIQUE(object_type, instance, account, native_key, ordinal),
 FOREIGN KEY(object_type, instance, account, native_key)
 REFERENCES objects(object_type, instance, account, native_key)
) WITHOUT ROWID;
CREATE TABLE links (
 source_type TEXT COLLATE BINARY NOT NULL,
 source_instance TEXT COLLATE BINARY NOT NULL,
 source_account TEXT COLLATE BINARY NOT NULL,
 source_key TEXT COLLATE BINARY NOT NULL,
 relation TEXT COLLATE BINARY NOT NULL,
 target_type TEXT COLLATE BINARY NOT NULL,
 target_instance TEXT COLLATE BINARY NOT NULL,
 target_account TEXT COLLATE BINARY NOT NULL,
 target_key TEXT COLLATE BINARY NOT NULL,
 PRIMARY KEY(source_type, source_instance, source_account, source_key, relation,
             target_type, target_instance, target_account, target_key),
 FOREIGN KEY(source_type, source_instance, source_account, source_key)
 REFERENCES objects(object_type, instance, account, native_key),
 FOREIGN KEY(target_type, target_instance, target_account, target_key)
 REFERENCES objects(object_type, instance, account, native_key)
) WITHOUT ROWID;
