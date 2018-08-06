#![allow(unused_imports)]

extern crate serde;
extern crate serde_json;
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

use serde::de::Deserialize;
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

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Cfg {
    pub contracts: Vec<ContractCfg>,
    pub library: Vec<String>
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ContractCfg {
    pub name: String,
    pub path: String,
    pub instances: Vec<ContractInstanceCfg>
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ContractInstanceCfg {
    pub address: String,
    pub params: Vec<ContractParamCfg>
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ContractParamCfg {
    pub name: String,
    pub value: serde_json::Value
}

#[derive(Debug)]
enum AbiValue {
    Single(String),
    Array(Vec<String>)
}

impl AbiValue {
    fn into_string(self) -> String {
        match self {
            AbiValue::Single(v) => v,
            AbiValue::Array(items) => format!("[{}]", items.join(","))
        }
    }
}

impl ContractParamCfg {
    pub fn to_abi_value(&self) -> AbiValue {
        use serde_json::Value;
        match self.value {
            Value::Bool(ref v) => AbiValue::Single(v.to_string()),
            Value::Number(ref v) => AbiValue::Single(format!("{}", v)),
            Value::String(ref v) => AbiValue::Single(remove_0x(v.as_str()).to_string()),
            Value::Array(ref items) => {
                let item_strings = items
                    .iter()
                    .map(|item| {
                        match item {
                            Value::Bool(v) => v.to_string(),
                            Value::Number(v) => format!("{}", v),
                            Value::String(v) => remove_0x(v.as_str()).to_string(),
                            _ => panic!("Invalid item: {:?}", item),
                        }
                    })
                    .collect::<Vec<_>>();
                AbiValue::Array(item_strings)
            },
            _ => panic!("Invalid value: {:?}", self.value),
        }
    }
}

fn load_json<'a, T>(path: &str, content: &'a mut String) -> T
where T: Deserialize<'a>
{
    let mut file = fs::File::open(path).unwrap();
    file.read_to_string(content).unwrap();
    serde_json::from_str(content).unwrap()
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
    cfg: ContractCfg,
    /// Contract bytecode (solc --bin)
    bytecode: Vec<u8>,
    /// Contract bytecode (solc --bin-runtime)
    bytecode_runtime: Vec<u8>,
    /// Function hashes
    hashes: HashMap<String, String>,
    /// ethabi Contract
    abi: ethabi::Contract,
}

impl Contract {
    /// load from path
    pub fn load(cfg: ContractCfg, library: &[&str], path: &str) -> Contract {
        fn check_command_output(output: &std::process::Output) {
            if !output.status.success() {
                panic!(
                    "\n[stderr]: {}\n[stdout]: {}",
                    String::from_utf8_lossy(output.stderr.as_slice()),
                    String::from_utf8_lossy(output.stdout.as_slice()),
                );
            }
        }

        let allow_paths = library.join(",");
        let output = Command::new("solc")
            .args(&[
                "--combined-json", "abi,bin,hashes,bin-runtime",
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
            .find(|(long_name, _)| long_name.ends_with(format!(":{}", cfg.name).as_str()))
            .map(|(_, contract)| contract.as_object().unwrap())
            .unwrap();

        let bytecode: Vec<u8> = contract["bin"]
            .as_str()
            .unwrap()
            .from_hex()
            .unwrap();
        let bytecode_runtime: Vec<u8> = contract["bin-runtime"]
            .as_str()
            .unwrap()
            .from_hex()
            .unwrap();
        let abi_str = contract["abi"].as_str().unwrap();
        let abi = ethabi::Contract::load(abi_str.as_bytes()).unwrap();
        let hashes: HashMap<String, String> = contract["hashes"]
            .as_object()
            .unwrap()
            .into_iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
            .collect();
        Contract {cfg, bytecode, bytecode_runtime, abi, hashes}
    }

    /// code + constructor-params
    pub fn data(&self, param_values: &[&str]) -> Vec<u8> {
        if let Some(ref constructor) = self.abi.constructor {
            let tokens: Vec<Token> = param_values
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
                            // println!("bytes32: {} => {:?}", param_value, s);
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

impl AccountInfo {
    fn storage_map(&self) -> HashMap<String, String> {
        let map: HashMap<_, _> = self.storage
            .clone()
            .into();

        map.into_iter()
            .filter(|(_, value)| value.0 > U256::zero())
            .map(|(key, value)| (fill_hex(&key), fill_hex(&value.0)))
            .collect()
    }
}

fn fill_hex(value: &U256) -> String {
    let value_hex = format!("{:x}", value);
    if value_hex.is_empty() {
        format!("0x00")
    } else if value_hex.len() % 2 == 1 {
        format!("0x0{}", value_hex)
    } else {
        format!("0x{}", value_hex)
    }
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

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
struct GenesisBlock {
    timestamp: u64,
    prevhash: String,
    alloc: HashMap<String, AllocItem>
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
struct AllocItem {
    nonce: String,
    code: String,
    storage: HashMap<String, String>,
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
            Arg::with_name("config")
                .short("c")
                .long("config")
                .takes_value(true)
                .required(true)
                .help("Contracts JSON config path")
        )
        .arg(
            Arg::with_name("genesis")
                .short("g")
                .long("genesis")
                .takes_value(true)
                .required(true)
                .help("Genesis JSON file path")
        )
        .get_matches();

    let genesis_path = matches.value_of("genesis").unwrap();
    let mut genesis_content = String::new();
    let genesis_value: GenesisBlock = load_json(genesis_path, &mut genesis_content);

    let config_path = matches.value_of("config").unwrap();
    let directory = matches.value_of("directory").unwrap_or_else(|| {
        std::path::Path::new(config_path)
            .parent()
            .unwrap()
            .to_str()
            .unwrap()
    });
    let mut cfg_content = String::new();
    let cfg: Cfg = load_json(config_path, &mut cfg_content);
    let library: Vec<PathBuf> = cfg.library
        .iter()
        .map(|path| {
            let mut full_path = PathBuf::from(directory.clone());
            full_path.push(path);
            full_path
        })
        .collect();
    let library: Vec<&str> = library
        .iter()
        .map(|p| p.to_str().unwrap())
        .collect();
    let mut contract_addresses: HashMap<String, String> = HashMap::new();

    let mut genesis = GenesisBlock {
        timestamp: 1533284582935,
        prevhash: "0x0000000000000000000000000000000000000000000000000000000000000000".to_string(),
        alloc: HashMap::new(),
    };

    let contracts: Vec<Contract> = cfg.contracts
        .into_iter()
        .map(|contract_cfg| {
            let mut path = PathBuf::from(&directory);
            path.push(&contract_cfg.path);
            Contract::load(contract_cfg, &library, path.to_str().unwrap())
        })
        .collect();
    let mut contract_dict: HashMap<&str, &Contract> = HashMap::new();

    for contract in &contracts {
        contract_dict.insert(contract.cfg.name.as_str(), &contract);
        for instance_cfg in &contract.cfg.instances {
            contract_addresses.insert(
                contract.cfg.name.clone(),
                instance_cfg.address.clone()
            );
            contract_dict.insert(
                instance_cfg.address.as_str(),
                &contract
            );
            let param_values: Vec<String> = if contract.cfg.name == "Permission" {
                instance_cfg.params
                    .iter()
                    .map(|param| {
                        let value = param.to_abi_value();
                        match param.name.as_str() {
                            "contracts" => {
                                if let AbiValue::Array(values) = value {
                                    let s = values
                                        .into_iter()
                                        .map(|value| {
                                            contract_addresses.get(&value).map(|s| remove_0x(s).to_string()).unwrap_or(value)
                                        })
                                        .collect::<Vec<_>>()
                                        .join(",");
                                    format!("[{}]", s)
                                } else {
                                    panic!("value should be array")
                                }
                            }
                            "functions" => {
                                if let AbiValue::Array(values) = value {
                                    let s = values
                                        .into_iter()
                                        .enumerate()
                                        .map(|(i, value)| {
                                            let contract_name = instance_cfg
                                                .params[1]
                                                .value
                                                .as_array()
                                                .unwrap()
                                                .get(i)
                                                .unwrap()
                                                .as_str()
                                                .unwrap()
                                                .to_string();
                                            if let Some(contract) = contract_dict.get(contract_name.as_str()) {
                                                contract.hashes.get(&value).map(|s| remove_0x(s).to_string()).unwrap_or(value)
                                            } else {
                                                value
                                            }
                                        })
                                        .collect::<Vec<_>>()
                                        .join(",");
                                    format!("[{}]", s)
                                } else {
                                    panic!("value should be array")
                                }
                            }
                            _ => value.into_string()
                        }
                    })
                    .collect()
            } else {
                instance_cfg.params
                    .iter()
                    .map(|param| param.to_abi_value().into_string())
                    .collect()
            };
            let param_values: Vec<&str> = param_values
                .iter()
                .map(|p| p.as_str())
                .collect();
            let data = contract.data(&param_values);
            if let Some(ref info) = gen_account(Rc::new(data)) {
                let alloc_item = AllocItem {
                    nonce: "1".to_string(),
                    code: format!("0x{}", info.code.to_hex::<String>()),
                    storage: info.storage_map()
                };
                genesis.alloc.insert(instance_cfg.address.clone(), alloc_item);
            }
            println!("=============================\n");
        }
    }

    let left = genesis;
    let right = genesis_value;
    assert_eq!(left.timestamp, right.timestamp);
    assert_eq!(left.prevhash, right.prevhash);
    for (address, left_item) in &left.alloc {
        println!(">> address: {}", address);
        let right_item = right.alloc.get(address).unwrap();
        assert_eq!(left_item.nonce, right_item.nonce);
        // assert_eq!(left_item.code, right_item.code);
        for (key, left_value) in &left_item.storage {
            println!(" > storage.key: {}, left={}", key, left_value);
            let right_value = right_item.storage.get(key).unwrap();
            assert_eq!(left_value, right_value);
        }
        for (key, right_value) in &right_item.storage {
            println!(" > storage.key: {}, right={}", key, right_value);
            let left_value = left_item.storage.get(key).unwrap();
            assert_eq!(left_value, right_value);
        }
        println!("=============================\n");
    }
}
