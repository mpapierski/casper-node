//! This crate contains types that contain the logic necessary to validate Casper implementation
//! correctness using external test fixtures.
//!
//! Casper test fixtures can contain multiple directories at the root level, which corresponds to a
//! test category. For example structure of files found inside `ABI` can differ from files in other
//! directories.
//!
//! Currently supported test fixtures:
//!
//! * [ABI](abi)

#[macro_use]
extern crate derive_more;

pub mod abi;
pub mod error;
pub mod test_case;
pub mod utils;

use std::{
    ffi::OsStr,
    fs::{self, File},
    io::BufReader,
    path::{Path, PathBuf},
};

use serde::de::DeserializeOwned;

use abi::ABIFixture;
use error::Error;

pub const ABI_TEST_FIXTURES: &str = "ABI";
const JSON_FILE_EXT: &str = "json";

#[derive(Debug)]
pub enum Fixture {
    /// ABI fixture.
    ABI {
        /// Name of the test fixture (taken from a file name).
        name: String,
        /// ABI fixture itself.
        fixture: ABIFixture,
    },
}

/// Loads a generic test fixture from a file with a reader based on a file extension.
///
/// Currently only JSON files are supported.
pub fn load_fixture<T: DeserializeOwned>(path: PathBuf) -> Result<T, Error> {
    let file = File::open(&path)?;
    let buffered_reader = BufReader::new(file);

    let fixture = match path.extension().and_then(OsStr::to_str) {
        Some(extension) if extension.to_ascii_lowercase() == JSON_FILE_EXT => {
            serde_json::from_reader(buffered_reader)?
        }
        Some(_) => return Err(Error::UnsupportedFormat(path)),
        None => return Err(Error::NoExtension(path)),
    };
    Ok(fixture)
}

/// A series of fixtures. One element represents a single structured file.
pub type TestFixtures = Vec<Fixture>;

/// Loads fixtures from a directory.
pub fn load_fixtures(path: &Path) -> Result<TestFixtures, Error> {
    let mut test_fixtures = TestFixtures::new();

    for entry in fs::read_dir(path)? {
        let entry = entry?;

        if !entry.metadata()?.is_dir() {
            continue;
        }

        let dir_entries = match entry.path().file_name() {
            Some(file_name) if file_name == ABI_TEST_FIXTURES => {
                utils::recursive_read_dir(&entry.path())?
            }
            None | Some(_) => continue,
        };

        for dir_entry in dir_entries {
            let dir_entry_path = dir_entry.path();
            let fixture = load_fixture(dir_entry_path.clone())?;
            let filename = dir_entry_path
                .file_stem()
                .and_then(OsStr::to_str)
                .ok_or_else(|| Error::NoStem(dir_entry_path.clone()))?;
            test_fixtures.push(Fixture::ABI {
                name: filename.to_string(),
                fixture,
            });
        }
    }
    Ok(test_fixtures)
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::Write};

    use borsh::{BorshSchema, BorshSerialize};
    use casper_execution_engine::core::engine_state::ExecutableDeployItem;
    use casper_types::{bytesrepr::Bytes, runtime_args, Contract, RuntimeArgs, StoredValue};

    #[test]
    fn gen_schema_for_stored_value() {
        // let container = StoredValue::schema_container();
        // // eprintln!("{:?}", container);
        // File::create("/tmp/stored_value.txt")
        //     .unwrap()
        //     .write_all(&format!("{:#?}", container).as_bytes())
        //     .unwrap();

        // let contract = Contract::new(ContractPackageHash::new([42;32]), ContractPackageHash::new([43;32]), {
        //     let mut keys = NamedKeys::new();
        //     keys.insert("named key 1", Key::Hash([44; 32])),
        //     keys,
        // }, {
        //     let mut eps = EntryPoints::new();
        //     eps.add_entry_point(EntryPoint::new());
        //     eps
        // }, ProtocolVersion::new(1, 0, 0));
    }

    #[test]
    fn deploy_item() {
        let args = runtime_args! {
            "hello" => "world",
        };
        let container = ExecutableDeployItem::ModuleBytes {
            module_bytes: Bytes::from(vec![0, 1, 2, 3, 4, 5]),
            args,
        };

        File::create("/tmp/executable_deploy_item.schema")
            .unwrap()
            .write_all(&borsh::to_vec(&container).unwrap())
            .unwrap();

        let blob = borsh::try_to_vec_with_schema(&container).unwrap();
        File::create("/tmp/module_bytes_with_schema.data")
            .unwrap()
            .write_all(&blob)
            .unwrap();
    }
}
