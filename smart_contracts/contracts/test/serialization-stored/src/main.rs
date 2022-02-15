#![no_std]
#![no_main]

#[macro_use]
extern crate alloc;

use alloc::{
    string::{String, ToString},
    vec::Vec,
};

use borsh::{BorshDeserialize, BorshSerialize, BorshSchema};
use casper_contract::{contract_api::{runtime, storage}, unwrap_or_revert::UnwrapOrRevert};
use casper_types::{
    contracts::{EntryPoint, EntryPoints, NamedKeys},
    CLType, CLTyped, EntryPointAccess, EntryPointType, Parameter, bytesrepr::{Bytes, ToBytes, self},
};

pub mod addressbook_capnp {
    include!(concat!(env!("OUT_DIR"), "/addressbook_capnp.rs"));
}

const ENTRY_FUNCTION_NAME: &str = "write_capnp";
const HASH_KEY_NAME: &str = "do_nothing_hash";
const PACKAGE_HASH_KEY_NAME: &str = "do_nothing_package_hash";
const ACCESS_KEY_NAME: &str = "do_nothing_access";
const CONTRACT_VERSION: &str = "contract_version";
const ARG_PURSE_NAME: &str = "purse_name";

#[derive(BorshSerialize, BorshDeserialize, BorshSchema)]
#[repr(u32)]
enum Type {
    Mobile,
    Home,
    Work,
}

impl ToBytes for Type {
    fn to_bytes(&self) -> Result<Vec<u8>, casper_types::bytesrepr::Error> {
        let tag = match self {
            Type::Mobile => 0u8,
            Type::Home => 1u8,
            Type::Work => 2u8,
        };

        tag.to_bytes()

    }

    fn serialized_length(&self) -> usize {
        1
    }
}

#[derive(BorshSerialize, BorshDeserialize, BorshSchema)]
struct PhoneNumber {
    number: String,
    r#type: Type,
}

impl ToBytes for PhoneNumber {
    fn to_bytes(&self) -> Result<Vec<u8>, casper_types::bytesrepr::Error> {
        let mut vec = bytesrepr::allocate_buffer(self)?;
        vec.extend(self.number.to_bytes()?);
        vec.extend(self.r#type.to_bytes()?);
        Ok(vec)
    }

    fn serialized_length(&self) -> usize {
        self.number.serialized_length() + self.r#type.serialized_length()
    }
}

#[derive(BorshSerialize, BorshDeserialize, BorshSchema)]
enum Employment {
    Unemployed,
    Employer(String),
    School(String),
    Employed,
}


impl ToBytes for Employment {
    fn to_bytes(&self) -> Result<Vec<u8>, casper_types::bytesrepr::Error> {
        let mut vec = bytesrepr::allocate_buffer(self)?;

        match self {
            Employment::Unemployed => vec.extend(0u8.to_bytes()?),
            Employment::Employer(s) => { vec.extend(1u8.to_bytes()?); vec.extend(s.to_bytes()?); },
            Employment::School(s) => { vec.extend(2u8.to_bytes()?); vec.extend(s.to_bytes()?); },
            Employment::Employed => { vec.extend(3u8.to_bytes()?); },
        }

        Ok(vec)
    }

    fn serialized_length(&self) -> usize {
        1 + match self {
            Employment::Unemployed => 0,
            Employment::Employer(s) => s.serialized_length(),
            Employment::School(s) => s.serialized_length(),
            Employment::Employed => 0,
        }
    }
}

#[derive(BorshSerialize, BorshDeserialize, BorshSchema)]
struct Person {
    id: u32,
    name: String,
    email: String,
    phones: Vec<PhoneNumber>,
    employment: Employment,
}

impl ToBytes for Person {
    fn to_bytes(&self) -> Result<Vec<u8>, casper_types::bytesrepr::Error> {
        let mut vec = bytesrepr::allocate_buffer(self)?;

        vec.extend(self.id.to_bytes()?);
        vec.extend(self.name.to_bytes()?);
        vec.extend(self.email.to_bytes()?);
        vec.extend(self.phones.to_bytes()?);
        vec.extend(self.employment.to_bytes()?);

        Ok(vec)
    }

    fn serialized_length(&self) -> usize {
        self.id.serialized_length() +
        self.name.serialized_length() +
        self.email.serialized_length() +
        self.phones.serialized_length() +
        self.employment.serialized_length()
    }
}

#[derive(BorshSerialize, BorshDeserialize, BorshSchema)]
struct AddressBook {
    people: Vec<Person>,
}


impl ToBytes for AddressBook {
    fn to_bytes(&self) -> Result<Vec<u8>, casper_types::bytesrepr::Error> {
        self.people.to_bytes()
    }

    fn serialized_length(&self) -> usize {
        self.people.serialized_length()
    }
}


use addressbook_capnp::{address_book, person};
use capnp::serialize_packed;

pub fn write_address_book() -> ::capnp::Result<Vec<u8>> {
    let mut message = ::capnp::message::Builder::new_default();
    {
        let address_book = message.init_root::<address_book::Builder>();

        let mut people = address_book.init_people(2);

        {
            let mut alice = people.reborrow().get(0);
            alice.set_id(123);
            alice.set_name("Alice");
            alice.set_email("alice@example.com");
            {
                let mut alice_phones = alice.reborrow().init_phones(1);
                alice_phones.reborrow().get(0).set_number("555-1212");
                alice_phones
                    .reborrow()
                    .get(0)
                    .set_type(person::phone_number::Type::Mobile);
            }
            alice.get_employment().set_school("MIT");
        }

        {
            let mut bob = people.get(1);
            bob.set_id(456);
            bob.set_name("Bob");
            bob.set_email("bob@example.com");
            {
                let mut bob_phones = bob.reborrow().init_phones(2);
                bob_phones.reborrow().get(0).set_number("555-4567");
                bob_phones
                    .reborrow()
                    .get(0)
                    .set_type(person::phone_number::Type::Home);
                bob_phones.reborrow().get(1).set_number("555-7654");
                bob_phones
                    .reborrow()
                    .get(1)
                    .set_type(person::phone_number::Type::Work);
            }
            bob.get_employment().set_unemployed(());
        }
    }

    let mut vec = Vec::new();
    serialize_packed::write_message(&mut vec, &message)?;
    Ok(vec)
}

fn make_address_book() -> AddressBook {
    let alice = Person {
        id: 123,
        name: "Alice".to_string(),
        email: "alice@example.com".to_string(),
        phones: vec![PhoneNumber {
            number: "555-1212".to_string(),
            r#type: Type::Mobile,
        }],
        employment: Employment::School("MIT".to_string()),
    };
    let bob = Person {
        id: 456,
        name: "Bob".to_string(),
        email: "bob@example.com".to_string(),
        phones: vec![
            PhoneNumber {
                number: "555-4567".to_string(),
                r#type: Type::Home,
            },
            PhoneNumber {
                number: "555-7654".to_string(),
                r#type: Type::Work,
            },
        ],
        employment: Employment::Unemployed,
    };

    AddressBook {
        people: vec![alice, bob],
    }
}

#[no_mangle]
pub extern "C" fn write_capnp() {
    let uref = runtime::get_key("storage").unwrap_or_revert().into_uref().unwrap_or_revert();
    let data = write_address_book().ok().unwrap_or_revert();
    storage::write(uref, Bytes::from(data));
}

#[no_mangle]
pub extern "C" fn write_borsh() {
    let uref = runtime::get_key("storage").unwrap_or_revert().into_uref().unwrap_or_revert();
    let address_book = make_address_book();
    let data = address_book.try_to_vec().ok().unwrap_or_revert();
    storage::write(uref, Bytes::from(data));
}


#[no_mangle]
pub extern "C" fn write_tobytes() {
    let uref = runtime::get_key("storage").unwrap_or_revert().into_uref().unwrap_or_revert();
    let address_book = make_address_book();
    let data = address_book.try_to_vec().ok().unwrap_or_revert();
    storage::write(uref, Bytes::from(data));
}

#[no_mangle]
pub extern "C" fn call() {
    let entry_points = {
        let mut entry_points = EntryPoints::new();
        let entry_point = EntryPoint::new(
            ENTRY_FUNCTION_NAME.to_string(),
            Vec::new(),
            CLType::Unit,
            EntryPointAccess::Public,
            EntryPointType::Contract,
        );
        entry_points.add_entry_point(entry_point);
        let entry_point = EntryPoint::new(
            "write_borsh",
            Vec::new(),
            CLType::Unit,
            EntryPointAccess::Public,
            EntryPointType::Contract,
        );
        entry_points.add_entry_point(entry_point);
        let entry_point = EntryPoint::new(
            "write_tobytes",
            Vec::new(),
            CLType::Unit,
            EntryPointAccess::Public,
            EntryPointType::Contract,
        );
        entry_points.add_entry_point(entry_point);
        entry_points
    };

    let named_keys = {
        let uref = storage::new_uref(());
        let mut named_keys = NamedKeys::new();
        named_keys.insert("storage".to_string(), uref.into());

        named_keys
    };

    let (contract_hash, contract_version) = storage::new_contract(
        entry_points,
        Some(named_keys),
        Some(PACKAGE_HASH_KEY_NAME.to_string()),
        Some(ACCESS_KEY_NAME.to_string()),
    );

    runtime::put_key(CONTRACT_VERSION, storage::new_uref(contract_version).into());
    runtime::put_key(HASH_KEY_NAME, contract_hash.into());
}
