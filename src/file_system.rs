use rocksdb;

pub struct FileSystem {
    db: rocksdb::DB,
}

impl FileSystem {
    pub fn new() -> FileSystem {
        let mut options = rocksdb::Options::default();

        options.create_if_missing(true);

        let db = rocksdb::DB::open(&options,"/tmp/file_system").unwrap();
        FileSystem { db }
    }

    pub fn write(&self, key: &str, value: &str) {
        self.db.put(key.as_bytes(), value.as_bytes()).unwrap();
    }

    pub fn read(&self, key: &str) -> Option<String> {
        match self.db.get(key.as_bytes()) {
            Ok(Some(value)) => Some(String::from_utf8(value).unwrap()),
            Ok(None) => None,
            Err(e) => panic!("Error while reading from database: {:?}", e),
        }
    }
    
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new() {
        let fs = FileSystem::new();
        assert_eq!(fs.read("test"), None);
    }

    #[test]
    fn test_write() {
        let fs = FileSystem::new();
        fs.write("test", "test_value");
        assert_eq!(fs.read("test"), Some("test_value".to_string()));
    }

    #[test]
    fn test_read() {
        let fs = FileSystem::new();
        fs.write("test", "test_value");
        assert_eq!(fs.read("test"), Some("test_value".to_string()));
    }
}