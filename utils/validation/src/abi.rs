use std::collections::BTreeMap;

use borsh::{self, maybestd::io, BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

use casper_types::{
    bytesrepr::{self, ToBytes},
    CLValue, Key, StoredValue, Transform, U512,
};

use crate::test_case::{Error, TestCase};

/// Representation of supported input value.
#[derive(Serialize, Deserialize, Debug, From)]
#[serde(tag = "type", content = "value")]
pub enum Input {
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    String(String),
    Bool(bool),
    U512(U512),
    CLValue(CLValue),
    Key(Key),
    Transform(Transform),
    StoredValue(StoredValue),
}

impl Input {
    pub fn borsh_serialized(&self) -> Result<Vec<u8>, io::Error> {
        let bytes = match self {
            Input::U8(value) => borsh::to_vec(value)?,
            Input::U16(value) => borsh::to_vec(value)?,
            Input::U32(value) => borsh::to_vec(value)?,
            Input::U64(value) => borsh::to_vec(value)?,
            Input::String(value) => borsh::to_vec(value)?,
            Input::Bool(value) => borsh::to_vec(value)?,
            Input::U512(value) => borsh::to_vec(value)?,
            Input::CLValue(value) => borsh::to_vec(value)?,
            Input::Key(value) => borsh::to_vec(value)?,
            Input::Transform(value) => borsh::to_vec(value)?,
            Input::StoredValue(value) => borsh::to_vec(value)?,
        };
        Ok(bytes)
    }
}

// impl ToBytes for Input {
//     fn to_bytes(&self) -> Result<Vec<u8>, bytesrepr::Error> {
//         match self {
//             Input::U8(value) => value.to_bytes(),
//             Input::U16(value) => value.to_bytes(),
//             Input::U32(value) => value.to_bytes(),
//             Input::U64(value) => value.to_bytes(),
//             Input::String(value) => value.to_bytes(),
//             Input::Bool(value) => value.to_bytes(),
//             Input::U512(value) => value.to_bytes(),
//             Input::CLValue(value) => value.to_bytes(),
//             Input::Key(value) => value.to_bytes(),
//             Input::Transform(value) => value.to_bytes(),
//             Input::StoredValue(value) => value.to_bytes(),
//         }
//     }

//     fn serialized_length(&self) -> usize {
//         match self {
//             Input::U8(value) => value.serialized_length(),
//             Input::U16(value) => value.serialized_length(),
//             Input::U32(value) => value.serialized_length(),
//             Input::U64(value) => value.serialized_length(),
//             Input::String(value) => value.serialized_length(),
//             Input::Bool(value) => value.serialized_length(),
//             Input::U512(value) => value.serialized_length(),
//             Input::CLValue(value) => value.serialized_length(),
//             Input::Key(value) => value.serialized_length(),
//             Input::Transform(value) => value.serialized_length(),
//             Input::StoredValue(value) => value.serialized_length(),
//         }
//     }
// }

/// Test case defines a list of inputs and an output.
#[derive(Serialize, Deserialize, Debug)]
pub struct ABITestCase {
    input: Vec<serde_json::Value>,
    output: String,
}

impl ABITestCase {
    pub fn from_inputs(inputs: Vec<Input>) -> Result<ABITestCase, Error> {
        // This is manually going through each input passed as we can't use `ToBytes for Vec<T>` as
        // the `output` would be a serialized collection.
        let mut truth = Vec::new();
        for input in &inputs {
            // Input::to_bytes uses static dispatch to call into each raw value impl.
            let mut generated_truth = input.borsh_serialized()?;
            truth.append(&mut generated_truth);
        }

        let input_values = inputs
            .into_iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(ABITestCase {
            input: input_values,
            output: hex::encode(truth),
        })
    }

    pub fn input(&self) -> Result<Vec<Input>, Error> {
        let mut res = Vec::new();
        for input_value in &self.input {
            let input: Input = serde_json::from_value(input_value.clone())?;
            res.push(input);
        }
        Ok(res)
    }

    pub fn output(&self) -> Result<Vec<u8>, Error> {
        let output = hex::decode(&self.output)?;
        Ok(output)
    }

    pub fn to_borsh_bytes(&self) -> Result<Vec<u8>, Error> {
        // let mut res = Vec::with_capacity(self.serialized_length()?);
        let mut res = Vec::new();

        for input in self.input()? {
            res.append(&mut input.borsh_serialized()?);
        }

        Ok(res)
    }

    // pub fn serialized_length(&self) -> Result<usize, Error> {
    //     Ok(self.input()?.iter().map(ToBytes::serialized_length).sum())
    // }
}

impl TestCase for ABITestCase {
    /// Compares input to output.
    ///
    /// This gets executed for each test case.
    fn run_test(&self) -> Result<(), Error> {
        // let serialized_length = self.serialized_length()?;
        let serialized_data = self.to_borsh_bytes()?;

        let output = self.output()?;

        // Serialized data should match the output
        if serialized_data != output {
            if serialized_data.len() == output.len() {
                for (i, (a, b)) in serialized_data.iter().zip(output.iter()).enumerate() {
                    if *a != *b {
                        eprintln!("first mismatch at {} ({} != {})", i, a, b);
                    }
                }
            }
            // eprintln!("{} {}", serialized_data.len(), output.len());
            return Err(Error::DataMismatch {
                actual: serialized_data,
                expected: output.to_vec(),
            });
        }

        // // Output from serialized_length should match the output data length
        // if serialized_length != output.len() {
        //     return Err(Error::LengthMismatch {
        //         expected: serialized_length,
        //         actual: output.len(),
        //     });
        // }

        Ok(())
    }
}

/// A fixture consists of multiple test cases.
#[derive(Serialize, Deserialize, Debug, From)]
pub struct ABIFixture(BTreeMap<String, ABITestCase>);

impl ABIFixture {
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn into_inner(self) -> BTreeMap<String, ABITestCase> {
        self.0
    }
}
