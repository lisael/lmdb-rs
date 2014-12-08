use libc::{c_uint, c_void, size_t};
use std::{mem, ptr, raw};
use std::kinds::marker;
use std::io::BufWriter;

use cursor::{RoCursor, RwCursor};
use database::Database;
use environment::Environment;
use error::{LmdbResult, lmdb_result};
use ffi;
use ffi::MDB_txn;
use flags::{DatabaseFlags, EnvironmentFlags, WriteFlags, MDB_RDONLY, MDB_RESERVE};

/// An LMDB transaction.
///
/// All database operations require a transaction.
pub trait Transaction<'env> {

    /// Returns a raw pointer to the underlying LMDB transaction.
    ///
    /// The caller **must** ensure that the pointer is not used after the lifetime of the
    /// transaction.
    fn txn(&self) -> *mut MDB_txn;
}

/// A read-only LMDB transaction.
///
/// All database read operations require a transaction.
pub trait ReadTransaction<'env> : Transaction<'env> {

    /// Opens a handle to a database which may contain at most one item per key.
    ///
    /// If `name` is `None`, then the returned handle will be for the default database.
    ///
    /// If `name` is not `None`, then the returned handle will be for a named database. In this
    /// case the environment must be configured to allow named databases through
    /// `EnvironmentBuilder::set_max_dbs`.
    ///
    /// The database handle will be private to the current transaction until the transaction is
    /// successfully committed. If the transaction is aborted the database handle will be closed
    /// automatically. After a successful commit the database handle will reside in the shared
    /// environment, and may be used by other transactions.
    ///
    /// A transaction that uses this function must finish (either commit or abort) before any other
    /// transaction may use the function.
    fn open_uniq_db(&self, name: Option<&str>, flags: DatabaseFlags) -> LmdbResult<Database<'env>> {
        Database::open_uniq(self, name, flags)
    }

    /// Opens a handle to a database which may contain multiple items per key.
    ///
    /// See `ReadTransaction::open_uniq_db` for parameter and usage information.
    fn open_dup_db(&self, name: Option<&str>, flags: DatabaseFlags) -> LmdbResult<Database<'env>> {
        Database::open_dup(self, name, flags)
    }

    /// Gets an item from a database.
    ///
    /// This function retrieves the data associated with the given key in the database. If the
    /// database supports duplicate keys (`MDB_DUPSORT`) then the first data item for the key will
    /// be returned. Retrieval of other items requires the use of `Transaction::cursor_get`.
    fn get<'txn: 'env, 'a>(&'txn self, database: Database, key: &'a [u8]) -> LmdbResult<&'txn [u8]> {
        let mut key_val: ffi::MDB_val = ffi::MDB_val { mv_size: key.len() as size_t,
                                                       mv_data: key.as_ptr() as *mut c_void };
        let mut data_val: ffi::MDB_val = ffi::MDB_val { mv_size: 0,
                                                        mv_data: ptr::null_mut() };
        unsafe {
            try!(lmdb_result(ffi::mdb_get(self.txn(),
                                          database.dbi(),
                                          &mut key_val,
                                          &mut data_val)));
            let slice: &'txn [u8] =
                mem::transmute(raw::Slice {
                    data: data_val.mv_data as *const u8,
                    len: data_val.mv_size as uint
                });
            Ok(slice)
        }
    }

    /// Open a new read-only cursor on the given database.
    fn open_read_cursor<'txn>(&'txn self, db: Database) -> LmdbResult<RoCursor<'txn>> {
        RoCursor::new(self, db)
    }

    /// Commits the transaction.
    ///
    /// Any pending operations will be saved.
    fn commit(self) -> LmdbResult<()> {
        unsafe {
            let result = lmdb_result(ffi::mdb_txn_commit(self.txn()));
            mem::forget(self);
            result
        }
    }

    /// Aborts the transaction.
    ///
    /// Any pending operations will not be saved.
    fn abort(self) {
        // Abort is called in the destructor
    }

    /// Gets the option flags for the given database in the transaction.
    fn db_flags(&self, db: Database) -> LmdbResult<DatabaseFlags> {
        let mut flags: c_uint = 0;
        unsafe {
            try!(lmdb_result(ffi::mdb_dbi_flags(self.txn(), db.dbi(), &mut flags)));
        }

        Ok(DatabaseFlags::from_bits_truncate(flags))
    }
}

/// A read-write LMDB transaction.
///
/// All database operations require a transaction.
pub trait WriteTransaction<'env> : ReadTransaction<'env> {

    /// Open a new read-write cursor on the given database.
    fn open_write_cursor<'txn>(&'txn mut self, db: Database) -> LmdbResult<RwCursor<'txn>> {
        RwCursor::new(self, db)
    }

    /// Stores an item into a database.
    ///
    /// This function stores key/data pairs in the database. The default behavior is to enter the
    /// new key/data pair, replacing any previously existing key if duplicates are disallowed, or
    /// adding a duplicate data item if duplicates are allowed (`MDB_DUPSORT`).
    fn put(&mut self,
           database: Database,
           key: &[u8],
           data: &[u8],
           flags: WriteFlags)
           -> LmdbResult<()> {
        let mut key_val: ffi::MDB_val = ffi::MDB_val { mv_size: key.len() as size_t,
                                                       mv_data: key.as_ptr() as *mut c_void };
        let mut data_val: ffi::MDB_val = ffi::MDB_val { mv_size: data.len() as size_t,
                                                        mv_data: data.as_ptr() as *mut c_void };
        unsafe {
            lmdb_result(ffi::mdb_put(self.txn(),
                                     database.dbi(),
                                     &mut key_val,
                                     &mut data_val,
                                     flags.bits()))
        }
    }

    /// Returns a `BufWriter` which can be used to write a value into the item at the given key
    /// and with the given length. The buffer must be completely filled by the caller.
    fn reserve<'txn>(&'txn mut self,
                         database: Database,
                         key: &[u8],
                         len: size_t,
                         flags: WriteFlags)
                         -> LmdbResult<BufWriter<'txn>> {
        let mut key_val: ffi::MDB_val = ffi::MDB_val { mv_size: key.len() as size_t,
                                                       mv_data: key.as_ptr() as *mut c_void };
        let mut data_val: ffi::MDB_val = ffi::MDB_val { mv_size: len,
                                                        mv_data: ptr::null_mut::<c_void>() };
        unsafe {
            try!(lmdb_result(ffi::mdb_put(self.txn(),
                                          database.dbi(),
                                          &mut key_val,
                                          &mut data_val,
                                          (flags | MDB_RESERVE).bits())));
            let slice: &'txn mut [u8] =
                mem::transmute(raw::Slice {
                    data: data_val.mv_data as *const u8,
                    len: data_val.mv_size as uint
                });

            Ok(BufWriter::new(slice))
        }
    }

    /// Deletes an item from a database.
    ///
    /// This function removes key/data pairs from the database. If the database does not support
    /// sorted duplicate data items (`MDB_DUPSORT`) the data parameter is ignored. If the database
    /// supports sorted duplicates and the data parameter is `None`, all of the duplicate data items
    /// for the key will be deleted. Otherwise, if the data parameter is `Some` only the matching
    /// data item will be deleted. This function will return `MDB_NOTFOUND` if the specified key/data
    /// pair is not in the database.
    fn del(&mut self,
               database: Database,
               key: &[u8],
               data: Option<&[u8]>)
               -> LmdbResult<()> {
        let mut key_val: ffi::MDB_val = ffi::MDB_val { mv_size: key.len() as size_t,
                                                       mv_data: key.as_ptr() as *mut c_void };
        let data_val: Option<ffi::MDB_val> =
            data.map(|data| ffi::MDB_val { mv_size: data.len() as size_t,
                                           mv_data: data.as_ptr() as *mut c_void });
        unsafe {
            lmdb_result(ffi::mdb_del(self.txn(),
                                     database.dbi(),
                                     &mut key_val,
                                     data_val.map(|mut data_val| &mut data_val as *mut _)
                                             .unwrap_or(ptr::null_mut())))
        }
    }
}

/// An LMDB read-only transaction.
pub struct RoTransaction<'env> {
    txn: *mut MDB_txn,
    _no_sync: marker::NoSync,
    _no_send: marker::NoSend,
    _contravariant: marker::ContravariantLifetime<'env>,
}

#[unsafe_destructor]
impl <'env> Drop for RoTransaction<'env> {
    fn drop(&mut self) {
        unsafe { ffi::mdb_txn_abort(self.txn) }
    }
}

impl <'env> RoTransaction<'env> {

    /// Creates a new read-only transaction in the given environment. Prefer using
    /// `Environment::begin_ro_txn`.
    #[doc(hidden)]
    pub fn new(env: &'env Environment) -> LmdbResult<RoTransaction<'env>> {
        let mut txn: *mut MDB_txn = ptr::null_mut();
        unsafe {
            try!(lmdb_result(ffi::mdb_txn_begin(env.env(),
                                                ptr::null_mut(),
                                                MDB_RDONLY.bits(),
                                                &mut txn)));
            Ok(RoTransaction {
                txn: txn,
                _no_sync: marker::NoSync,
                _no_send: marker::NoSend,
                _contravariant: marker::ContravariantLifetime::<'env>,
            })
        }
    }
}

impl <'env> Transaction<'env> for RoTransaction<'env> {
    fn txn(&self) -> *mut MDB_txn {
        self.txn
    }
}

impl <'env> ReadTransaction<'env> for RoTransaction<'env> { }

/// An LMDB read-write transaction.
pub struct RwTransaction<'env> {
    txn: *mut MDB_txn,
    _no_sync: marker::NoSync,
    _no_send: marker::NoSend,
    _contravariant: marker::ContravariantLifetime<'env>,
}

#[unsafe_destructor]
impl <'env> Drop for RwTransaction<'env> {
    fn drop(&mut self) {
        unsafe { ffi::mdb_txn_abort(self.txn) }
    }
}

impl <'env> RwTransaction<'env> {

    /// Creates a new read-write transaction in the given environment. Prefer using
    /// `Environment::begin_ro_txn`.
    #[doc(hidden)]
    pub fn new(env: &'env Environment) -> LmdbResult<RwTransaction<'env>> {
        let mut txn: *mut MDB_txn = ptr::null_mut();
        unsafe {
            try!(lmdb_result(ffi::mdb_txn_begin(env.env(),
                                                ptr::null_mut(),
                                                EnvironmentFlags::empty().bits(),
                                                &mut txn)));
            Ok(RwTransaction {
                txn: txn,
                _no_sync: marker::NoSync,
                _no_send: marker::NoSend,
                _contravariant: marker::ContravariantLifetime::<'env>,
            })
        }
    }
}

impl <'env> Transaction<'env> for RwTransaction<'env> {
    fn txn(&self) -> *mut MDB_txn {
        self.txn
    }
}

impl <'env> ReadTransaction<'env> for RwTransaction<'env> { }
impl <'env> WriteTransaction<'env> for RwTransaction<'env> { }

#[cfg(test)]
mod test {

    use std::io;
    use std::sync::{Arc, Barrier, Future};

    use environment::*;
    use flags::*;
    use super::*;

    #[test]
    fn test_open_db() {
        let dir = io::TempDir::new("test").unwrap();
        let env = Environment::new().set_max_dbs(10)
                                    .open(dir.path(), io::USER_RWX)
                                    .unwrap();
        {
            let txn = env.begin_write_txn().unwrap();
            assert!(txn.open_uniq_db(None, DatabaseFlags::empty()).is_ok());
            assert!(txn.commit().is_ok());
        } {
            let txn = env.begin_read_txn().unwrap();
            assert!(txn.open_uniq_db(Some("testdb"), DatabaseFlags::empty()).is_err())
        } {
            let txn = env.begin_read_txn().unwrap();
            txn.open_uniq_db(Some("testdb"), MDB_CREATE).unwrap();
            assert!(txn.commit().is_ok());
        } {
            let txn = env.begin_read_txn().unwrap();
            assert!(txn.open_uniq_db(Some("testdb"), DatabaseFlags::empty()).is_ok())
        }
    }

    #[test]
    fn test_put_get_del() {
        let dir = io::TempDir::new("test").unwrap();
        let env = Environment::new().open(dir.path(), io::USER_RWX).unwrap();

        let mut txn = env.begin_write_txn().unwrap();
        let db = txn.open_uniq_db(None, DatabaseFlags::empty()).unwrap();
        txn.put(db, b"key1", b"val1", WriteFlags::empty()).unwrap();
        txn.put(db, b"key2", b"val2", WriteFlags::empty()).unwrap();
        txn.put(db, b"key3", b"val3", WriteFlags::empty()).unwrap();
        txn.commit().unwrap();

        let mut txn = env.begin_write_txn().unwrap();
        assert_eq!(b"val1", txn.get(db, b"key1").unwrap());
        assert_eq!(b"val2", txn.get(db, b"key2").unwrap());
        assert_eq!(b"val3", txn.get(db, b"key3").unwrap());
        assert!(txn.get(db, b"key").is_err());

        txn.del(db, b"key1", None).unwrap();
        assert!(txn.get(db, b"key1").is_err());
    }

    #[test]
    fn test_reserve() {
        let dir = io::TempDir::new("test").unwrap();
        let env = Environment::new().open(dir.path(), io::USER_RWX).unwrap();

        let mut txn = env.begin_write_txn().unwrap();
        let db = txn.open_uniq_db(None, DatabaseFlags::empty()).unwrap();
        {
            let mut writer = txn.reserve(db, b"key1", 4, WriteFlags::empty()).unwrap();
            writer.write(b"val1").unwrap();
        }
        txn.commit().unwrap();

        let mut txn = env.begin_write_txn().unwrap();
        assert_eq!(b"val1", txn.get(db, b"key1").unwrap());
        assert!(txn.get(db, b"key").is_err());

        txn.del(db, b"key1", None).unwrap();
        assert!(txn.get(db, b"key1").is_err());
    }

    #[test]
    fn test_close_database() {
        let dir = io::TempDir::new("test").unwrap();
        let env = Arc::new(Environment::new()
                                       .set_max_dbs(10)
                                       .open(dir.path(), io::USER_RWX)
                                       .unwrap());

        let db1 = {
            let txn = env.begin_write_txn().unwrap();
            let db = txn.open_uniq_db(Some("db"), MDB_CREATE).unwrap();
            txn.commit().unwrap();
            db
        };

        let db2 = {
            let txn = env.begin_read_txn().unwrap();
            let db = txn.open_uniq_db(Some("db"), DatabaseFlags::empty()).unwrap();
            txn.commit().unwrap();
            db
        };

        // Check that database handles are reused properly
        assert!(db1.dbi() == db2.dbi());

        {
            let mut txn = env.begin_write_txn().unwrap();
            txn.put(db1, b"key1", b"val1", WriteFlags::empty()).unwrap();
            assert!(txn.commit().is_ok());
        }

        unsafe { env.close_db(db1) };

        {
            let mut txn = env.begin_write_txn().unwrap();
            assert!(txn.put(db1, b"key2", b"val2", WriteFlags::empty()).is_err());
        }
    }

    #[test]
    fn test_concurrent_readers_single_writer() {
        let dir = io::TempDir::new("test").unwrap();
        let env: Arc<Environment> = Arc::new(Environment::new().open(dir.path(), io::USER_RWX).unwrap());

        let open_db_txn = env.begin_read_txn().unwrap();
        let db = open_db_txn.open_uniq_db(None, DatabaseFlags::empty()).unwrap();
        open_db_txn.commit().unwrap();

        let n = 10u; // Number of concurrent readers
        let barrier = Arc::new(Barrier::new(n + 1));
        let mut futures: Vec<Future<bool>> = Vec::with_capacity(n);

        let key = b"key";
        let val = b"val";

        for _ in range(0, n) {
            let reader_env = env.clone();
            let reader_barrier = barrier.clone();

            futures.push(Future::spawn(proc() {
                {
                    let txn = reader_env.begin_read_txn().unwrap();
                    assert!(txn.get(db, key).is_err());
                    txn.abort();
                }
                reader_barrier.wait();
                reader_barrier.wait();
                {
                    let txn = reader_env.begin_read_txn().unwrap();
                    txn.get(db, key).unwrap() == val
                }
            }));
        }

        let mut txn = env.begin_write_txn().unwrap();
        barrier.wait();
        txn.put(db, key, val, WriteFlags::empty()).unwrap();
        txn.commit().unwrap();
        barrier.wait();

        assert!(futures.iter_mut().all(|b| b.get()))
    }

    #[test]
    fn test_concurrent_writers() {
        let dir = io::TempDir::new("test").unwrap();
        let env = Arc::new(Environment::new().open(dir.path(), io::USER_RWX).unwrap());

        let open_db_txn = env.begin_read_txn().unwrap();
        let db = open_db_txn.open_uniq_db(None, DatabaseFlags::empty()).unwrap();
        open_db_txn.commit().unwrap();

        let n = 10u; // Number of concurrent writers
        let mut futures: Vec<Future<bool>> = Vec::with_capacity(n);

        let key = "key";
        let val = "val";

        for i in range(0, n) {
            let writer_env = env.clone();

            futures.push(Future::spawn(proc() {
                let mut txn = writer_env.begin_write_txn().unwrap();
                txn.put(db,
                        format!("{}{}", key, i).as_bytes(),
                        format!("{}{}", val, i).as_bytes(),
                        WriteFlags::empty())
                    .unwrap();
                txn.commit().is_ok()
            }));
        }
        assert!(futures.iter_mut().all(|b| b.get()));

        let txn = env.begin_read_txn().unwrap();

        for i in range(0, n) {
            assert_eq!(
                format!("{}{}", val, i).as_bytes(),
                txn.get(db, format!("{}{}", key, i).as_bytes()).unwrap());
        }
    }
}
