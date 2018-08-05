#![allow(unused_imports)]

extern crate serde;
extern crate serde_json;
extern crate serde_yaml;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate lazy_static;
extern crate sputnikvm;
extern crate sputnikvm_network_classic;
extern crate sputnikvm_stateful;
extern crate block;
extern crate trie;
extern crate rand;
extern crate sha3;
extern crate bigint;
extern crate rustc_hex;
extern crate clap;
extern crate ethabi;

use sha3::{Digest, Keccak256};
use bigint::{H256, U256, M256, Address, Gas};
use sputnikvm::{ValidTransaction, VM, SeqTransactionVM, HeaderParams, VMStatus};
use sputnikvm_network_classic::MainnetEIP160Patch;
use sputnikvm_stateful::{MemoryStateful, LiteralAccount};
use block::TransactionAction;
use trie::{Database, MemoryDatabase};
use std::collections::HashMap;
use std::str;
use std::str::FromStr;
use std::rc::Rc;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use rand::Rng;
use rustc_hex::{FromHex, ToHex};
use sputnikvm::{
    Storage,
    AccountChange,
};
use clap::{App, Arg};
use std::io::Read;
use ethabi::{
    param_type:: {
        ParamType,
    },
    token::{
        Token,
        Tokenizer,
        StrictTokenizer,
        LenientTokenizer
    },
};

fn load_yaml(path: &str) -> serde_yaml::Value {
    let mut content = String::new();
    let mut file = fs::File::open(path).unwrap();
    file.read_to_string(&mut content).unwrap();
    serde_yaml::from_str(&content).unwrap()
}

fn _load_json(path: &str) -> serde_json::Value {
    let mut content = String::new();
    let mut file = fs::File::open(path).unwrap();
    file.read_to_string(&mut content).unwrap();
    serde_json::from_str(&content).unwrap()
}

#[inline]
pub fn remove_0x(hex: &str) -> &str {
    {
        let tmp = hex.as_bytes();
        if tmp[..2] == b"0x"[..] || tmp[..2] == b"0X"[..] {
            return str::from_utf8(&tmp[2..]).unwrap();
        }
    }
    hex
}

#[derive(Debug)]
struct Contract {
    /// Contract name
    name: String,
    /// Source code file path
    path: Option<String>,
    /// Contract address
    address: Option<Address>,
    /// Contract bytecode (solc --bin)
    bytecode: Vec<u8>,
    // /// Contract bytecode runtime (solc --bin-runtime)
    // bytecode_runtime: Vec<u8>,
    /// ethabi Contract
    abi: ethabi::Contract,
    // /// userdoc
    // userdoc: HashMap<String, String>,
}

impl Contract {
    /// load from path
    pub fn load(common_files: &[&str], path: &str, name: &str) -> Contract {
        fn check_command_output(output: &std::process::Output) {
            if !output.status.success() {
                panic!(
                    "\n[stderr]: {}\n[stdout]: {}",
                    String::from_utf8_lossy(output.stderr.as_slice()),
                    String::from_utf8_lossy(output.stdout.as_slice()),
                );
            }
        }

        let allow_paths = common_files.join(",");
        let output = Command::new("solc")
            .args(&[
                "--combined-json", "abi,bin,bin-runtime,userdoc",
                "--allow-paths", allow_paths.as_str(),
                path,
            ])
            .output()
            .expect("Failed call solc");
        check_command_output(&output);

        let data: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let contract = data["contracts"]
            .as_object()
            .unwrap()
            .into_iter()
            .find(|(long_name, _)| long_name.ends_with(format!(":{}", name).as_str()))
            .map(|(_, contract)| contract.as_object().unwrap())
            .unwrap();

        let bytecode: Vec<u8> = contract["bin"]
            .as_str()
            .unwrap()
            .from_hex()
            .unwrap();
        let abi_str = contract["abi"].as_str().unwrap();
        let abi = ethabi::Contract::load(abi_str.as_bytes()).unwrap();
        Contract {
            bytecode,
            abi,
            name: name.to_string(),
            path: Some(path.to_string()),
            address: None,
        }
    }

    /// code + constructor-params
    pub fn data(&self, params: &[&str]) -> Vec<u8> {
        if let Some(ref constructor) = self.abi.constructor {
            let tokens: Vec<Token> = params
                .iter()
                .enumerate()
                .map(|(i, param_value)| {
                    let param_type = &constructor.inputs[i].kind;
                    match param_type {
                        ParamType::FixedBytes(_) => {
                            let mut bytes = [0u8; 32];
                            let value_bytes = param_value.as_bytes();
                            bytes[0..value_bytes.len()].copy_from_slice(value_bytes);
                            let s: String = bytes[..].to_hex();
                            println!("bytes32: {} => {:?}", param_value, s);
                            StrictTokenizer::tokenize(param_type, s.as_str()).unwrap()
                        }
                        _ => {
                            LenientTokenizer::tokenize(param_type, param_value).unwrap()
                        }
                    }
                })
                .collect();
            constructor.encode_input(self.bytecode.clone(), &tokens).unwrap()
        } else {
            self.bytecode.clone()
        }
    }
}

#[derive(Debug)]
struct AccountInfo {
    pub nonce: U256,
    pub address: Address,
    pub balance: U256,
    pub storage: Storage,
    pub code: Rc<Vec<u8>>,
}

fn gen_account(data: Rc<Vec<u8>>) -> Option<AccountInfo> {
    let database = MemoryDatabase::default();
    let mut stateful = MemoryStateful::empty(&database);

    let vm: SeqTransactionVM<MainnetEIP160Patch> = stateful.execute(ValidTransaction {
        caller: None,
        gas_price: Gas::zero(),
        gas_limit: Gas::from(10000000000000u64),
        action: TransactionAction::Create,
        value: U256::zero(),
        input: data.clone(),
        nonce: U256::one(),
    }, HeaderParams {
        beneficiary: Address::default(),
        timestamp: 1533284582935,
        number: U256::zero(),
        difficulty: U256::zero(),
        gas_limit: Gas::max_value()
    }, &[]);
    match vm.status() {
        VMStatus::ExitedOk => (),
        err => panic!("error: {:?}", err),
    }

    for account in vm.current_state().unwrap().account_state.accounts() {
        if let AccountChange::Create {
            nonce, address, balance, storage, code
        } = account.clone()
        {
            return Some(AccountInfo {nonce, address, balance, storage, code});
        }
    }
    None
}

fn main() {
    let matches = App::new("create-genesis")
        .arg(
            Arg::with_name("directory")
                .long("directoryr")
                .takes_value(true)
                .help("Contracts directory")
        )
        .arg(
            Arg::with_name("contracts")
                .short("c")
                .long("contracts")
                .takes_value(true)
                .required(true)
                .help("Contracts YAML config path")
        )
        .arg(
            Arg::with_name("data")
                .short("d")
                .long("data")
                .takes_value(true)
                .required(true)
                .help("Initialize data(for solidity constructor arguments) YAML config path")
        )
        .get_matches();

    let contracts_path = matches.value_of("contracts").unwrap();
    let contracts = load_yaml(contracts_path);
    let data = load_yaml(matches.value_of("data").unwrap());
    let directory = matches.value_of("directory").unwrap_or_else(|| {
        std::path::Path::new(contracts_path)
            .parent()
            .unwrap()
            .to_str()
            .unwrap()
    });
    let mut common_directory: PathBuf = PathBuf::new();
    common_directory.push(directory);
    common_directory.push("common");
    let common_files: Vec<PathBuf> = common_directory
        .read_dir()
        .unwrap()
        .filter(|rv| rv.is_ok())
        .map(|rv| rv.unwrap())
        .filter(|entry| {
            entry
                .file_type()
                .map(|t| t.is_file())
                .ok()
                .unwrap_or(false)
        })
        .map(|entry| {
            let mut path = common_directory.clone();
            path.push(entry.file_name());
            path
        })
        .collect();
    let common_file_strs: Vec<&str> = common_files
        .iter()
        .map(|p| p.to_str().unwrap())
        .collect();

    contracts["NormalContracts"]
        .as_sequence()
        .unwrap()
        .into_iter()
        .map(|v| v.as_mapping().unwrap().iter().next().unwrap())
        .map(|(name, config)| {
            let name = name.as_str().unwrap();
            let address = config["address"].as_str().unwrap();
            let file_path: &str = config["file"].as_str().unwrap();
            let abs_file: PathBuf = Path::new(directory).join(file_path);
            let abs_file_path: &str = abs_file.to_str().unwrap();
            println!("name={}, address={}, file={}", name, address, abs_file_path);
            let contract = Contract::load(&common_file_strs, abs_file_path, name);
            let param_values = data["Contracts"]
                .as_sequence()
                .unwrap()
                .iter()
                .map(|item| {
                    item
                        .as_mapping()
                        .unwrap()
                        .iter()
                        .next()
                        .unwrap()
                })
                .find(|(key, _)| key.as_str().unwrap() == name)
                .map(|(_, values)| {
                    values
                        .as_sequence()
                        .unwrap()
                        .iter()
                        .map(|value| {
                            value
                                .as_mapping()
                                .unwrap()
                                .iter()
                                .next()
                                .map(|(key, value)| {
                                    println!("key={:?}, value={:?}", key, value);
                                    use serde_yaml::Value;
                                    match value {
                                        Value::Bool(v) => v.to_string(),
                                        Value::Number(v) => format!("{}", v),
                                        Value::String(v) => remove_0x(v.as_str()).to_string(),
                                        Value::Sequence(items) => {
                                            let items_string = items
                                                .iter()
                                                .map(|item| {
                                                    match item {
                                                        Value::Bool(v) => v.to_string(),
                                                        Value::Number(v) => format!("{}", v),
                                                        Value::String(v) => remove_0x(v.as_str()).to_string(),
                                                        _ => panic!("Invalid item: {:?}", item),
                                                    }
                                                })
                                                .collect::<Vec<_>>()
                                                .join(",");
                                            format!("[{}]", items_string)
                                        },
                                        _ => panic!("Invalid value: {:?}", value),
                                    }
                                })
                                .unwrap()
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or(vec![]);

            println!("values: {:?}", param_values);
            let param_strs: Vec<&str> = param_values
                .iter()
                .map(|s| s.as_str())
                .collect();
            gen_account(Rc::new(contract.data(param_strs.as_slice())))
        })
        .for_each(|info| {
            println!("account info={:#?}", info.map(|v| v.storage));
            println!("=============================\n");
        });
}
