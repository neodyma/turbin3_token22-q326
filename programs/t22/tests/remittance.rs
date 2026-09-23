use anchor_lang::{
    prelude::{Clock, Pubkey},
    solana_program::{instruction::Instruction, system_program},
    InstructionData, ToAccountMetas,
};
use anchor_spl::token_interface::spl_token_2022::{
    extension::{
        default_account_state::DefaultAccountState,
        metadata_pointer::MetadataPointer,
        mint_close_authority::MintCloseAuthority,
        transfer_fee::{instruction as fee_instruction, TransferFeeAmount, TransferFeeConfig},
        BaseStateWithExtensions, ExtensionType, StateWithExtensions,
    },
    instruction::{burn, close_account, initialize_account3, mint_to},
    state::{Account as TokenAccountState, AccountState, Mint as MintState},
    ID as TOKEN_2022_PROGRAM_ID,
};
use litesvm::{types::TransactionResult, LiteSVM};
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_transaction::Transaction;
use spl_token_metadata_interface::state::TokenMetadata;
use t22::{accounts, instruction, RemittanceMintArgs, ID};

const DECIMALS: u8 = 2;
const FEE_BPS: u16 = 250;
const MAX_FEE: u64 = 1_000;

struct Fixture {
    svm: LiteSVM,
    issuer: Keypair,
    freeze: Keypair,
    close: Keypair,
    fee_config: Keypair,
    withdraw: Keypair,
    outsider: Keypair,
    mint: Keypair,
}

impl Fixture {
    fn new() -> Self {
        let mut svm = LiteSVM::new();
        let program_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/deploy/t22.so");
        svm.add_program_from_file(ID, program_path).unwrap();
        let issuer = Keypair::new();
        let freeze = Keypair::new();
        let close = Keypair::new();
        let fee_config = Keypair::new();
        let withdraw = Keypair::new();
        let outsider = Keypair::new();
        let mint = Keypair::new();
        svm.airdrop(&issuer.pubkey(), 10_000_000_000).unwrap();
        Self {
            svm,
            issuer,
            freeze,
            close,
            fee_config,
            withdraw,
            outsider,
            mint,
        }
    }

    fn args(&self) -> RemittanceMintArgs {
        RemittanceMintArgs {
            decimals: DECIMALS,
            transfer_fee_basis_points: FEE_BPS,
            maximum_fee: MAX_FEE,
            freeze_authority: self.freeze.pubkey(),
            close_authority: self.close.pubkey(),
            fee_config_authority: self.fee_config.pubkey(),
            withdraw_withheld_authority: self.withdraw.pubkey(),
            name: "Remit USD".into(),
            symbol: "RUSD".into(),
            uri: "https://example.com/remit.json".into(),
        }
    }

    fn create_base_mint(&mut self) {
        let ix = Instruction {
            program_id: ID,
            accounts: accounts::CreateRemittanceMint {
                payer: self.issuer.pubkey(),
                mint: self.mint.pubkey(),
                token_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: instruction::CreateBaseMint { args: self.args() }.data(),
        };
        expect_success(send(&mut self.svm, &self.issuer, &[ix], &[&self.mint]));
    }

    fn token_account(&mut self, owner: &Pubkey) -> Pubkey {
        let token_account = Keypair::new();
        let len = ExtensionType::try_calculate_account_len::<TokenAccountState>(&[
            ExtensionType::TransferFeeAmount,
        ])
        .unwrap();
        let rent = self.svm.minimum_balance_for_rent_exemption(len);
        expect_success(send(
            &mut self.svm,
            &self.issuer,
            &[
                solana_system_interface::instruction::create_account(
                    &self.issuer.pubkey(),
                    &token_account.pubkey(),
                    rent,
                    len as u64,
                    &TOKEN_2022_PROGRAM_ID,
                ),
                initialize_account3(
                    &TOKEN_2022_PROGRAM_ID,
                    &token_account.pubkey(),
                    &self.mint.pubkey(),
                    owner,
                )
                .unwrap(),
            ],
            &[&token_account],
        ));
        token_account.pubkey()
    }

    fn thaw(&mut self, token_account: Pubkey, signer: &Keypair) -> TransactionResult {
        let ix = Instruction {
            program_id: ID,
            accounts: accounts::ThawRemittanceAccount {
                token_account,
                mint: self.mint.pubkey(),
                freeze_authority: signer.pubkey(),
                token_program: TOKEN_2022_PROGRAM_ID,
            }
            .to_account_metas(None),
            data: instruction::ThawRemittanceAccount {}.data(),
        };
        send(&mut self.svm, &self.issuer, &[ix], &[signer])
    }

    fn transfer(&mut self, source: Pubkey, destination: Pubkey, amount: u64) -> TransactionResult {
        let ix = Instruction {
            program_id: ID,
            accounts: accounts::TransferWithFee {
                source,
                mint: self.mint.pubkey(),
                destination,
                owner: self.issuer.pubkey(),
                token_program: TOKEN_2022_PROGRAM_ID,
            }
            .to_account_metas(None),
            data: instruction::TransferWithFee { amount }.data(),
        };
        send(&mut self.svm, &self.issuer, &[ix], &[])
    }

    fn token_state(&self, address: Pubkey) -> (u64, AccountState, u64) {
        let account = self.svm.get_account(&address).unwrap();
        let state = StateWithExtensions::<TokenAccountState>::unpack(&account.data).unwrap();
        let withheld = state
            .get_extension::<TransferFeeAmount>()
            .map(|extension| u64::from(extension.withheld_amount))
            .unwrap_or(0);
        (state.base.amount, state.base.state, withheld)
    }
}

fn send(
    svm: &mut LiteSVM,
    payer: &Keypair,
    instructions: &[Instruction],
    extra_signers: &[&Keypair],
) -> TransactionResult {
    svm.expire_blockhash();
    let mut signers = vec![payer];
    signers.extend_from_slice(extra_signers);
    let mut tx = Transaction::new_unsigned(Message::new(instructions, Some(&payer.pubkey())));
    tx.try_sign(&signers, svm.latest_blockhash()).unwrap();
    svm.send_transaction(tx)
}

fn expect_success(result: TransactionResult) {
    if let Err(error) = result {
        panic!(
            "transaction failed: {:?}\nlogs: {:#?}",
            error.err, error.meta.logs
        );
    }
}

#[test]
fn base_mint_lifecycle() {
    let mut fixture = Fixture::new();
    fixture.create_base_mint();
    let mint_account = fixture.svm.get_account(&fixture.mint.pubkey()).unwrap();
    let mint = StateWithExtensions::<MintState>::unpack(&mint_account.data).unwrap();
    let extensions = mint.get_extension_types().unwrap();
    assert_eq!(
        extensions,
        vec![
            ExtensionType::MintCloseAuthority,
            ExtensionType::TransferFeeConfig,
            ExtensionType::MetadataPointer,
            ExtensionType::DefaultAccountState,
            ExtensionType::TokenMetadata,
        ]
    );
    let metadata = mint.get_variable_len_extension::<TokenMetadata>().unwrap();
    assert_eq!(metadata.mint, fixture.mint.pubkey());
    assert_eq!(metadata.name, "Remit USD");
    assert_eq!(metadata.symbol, "RUSD");
    assert_eq!(metadata.uri, "https://example.com/remit.json");
    assert_eq!(
        Option::<Pubkey>::from(metadata.update_authority),
        Some(fixture.issuer.pubkey())
    );
    let pointer = mint.get_extension::<MetadataPointer>().unwrap();
    assert_eq!(
        Option::<Pubkey>::from(pointer.metadata_address),
        Some(fixture.mint.pubkey())
    );
    assert_eq!(
        mint.get_extension::<DefaultAccountState>().unwrap().state,
        AccountState::Frozen as u8
    );
    assert_eq!(
        Option::<Pubkey>::from(
            mint.get_extension::<MintCloseAuthority>()
                .unwrap()
                .close_authority
        ),
        Some(fixture.close.pubkey())
    );
    let fee = mint.get_extension::<TransferFeeConfig>().unwrap();
    assert_eq!(
        Option::<Pubkey>::from(fee.transfer_fee_config_authority),
        Some(fixture.fee_config.pubkey())
    );
    assert_eq!(
        Option::<Pubkey>::from(fee.withdraw_withheld_authority),
        Some(fixture.withdraw.pubkey())
    );
    assert_eq!(fee.calculate_epoch_fee(0, 10_000), Some(250));
    let fixed_len = ExtensionType::try_calculate_account_len::<MintState>(&[
        ExtensionType::TransferFeeConfig,
        ExtensionType::MetadataPointer,
        ExtensionType::DefaultAccountState,
        ExtensionType::MintCloseAuthority,
    ])
    .unwrap();
    assert_eq!(
        mint_account.data.len() + 8,
        fixed_len + metadata.tlv_size_of().unwrap()
    );
    assert!(
        mint_account.lamports
            >= fixture
                .svm
                .minimum_balance_for_rent_exemption(mint_account.data.len())
    );
    assert_eq!(
        mint.base.freeze_authority,
        Some(fixture.freeze.pubkey()).into()
    );
    drop(mint_account);

    let source = fixture.token_account(&fixture.issuer.pubkey());
    let destination = fixture.token_account(&fixture.outsider.pubkey());
    let treasury = fixture.token_account(&fixture.withdraw.pubkey());
    assert_eq!(fixture.token_state(source).1, AccountState::Frozen);
    assert_eq!(fixture.token_state(destination).1, AccountState::Frozen);

    let outsider = Keypair::try_from(fixture.outsider.to_bytes().as_slice()).unwrap();
    assert!(fixture.thaw(source, &outsider).is_err());
    assert_eq!(fixture.token_state(source).1, AccountState::Frozen);
    let freeze = Keypair::try_from(fixture.freeze.to_bytes().as_slice()).unwrap();
    expect_success(fixture.thaw(source, &freeze));
    expect_success(fixture.thaw(destination, &freeze));
    expect_success(fixture.thaw(treasury, &freeze));

    expect_success(send(
        &mut fixture.svm,
        &fixture.issuer,
        &[mint_to(
            &TOKEN_2022_PROGRAM_ID,
            &fixture.mint.pubkey(),
            &source,
            &fixture.issuer.pubkey(),
            &[],
            100_000,
        )
        .unwrap()],
        &[],
    ));
    expect_success(fixture.transfer(source, destination, 10_000));
    assert_eq!(
        fixture.token_state(source),
        (90_000, AccountState::Initialized, 0)
    );
    assert_eq!(
        fixture.token_state(destination),
        (9_750, AccountState::Initialized, 250)
    );
    assert!(fixture.transfer(destination, source, 100).is_err());
    assert_eq!(
        fixture.token_state(destination),
        (9_750, AccountState::Initialized, 250)
    );

    let newer = fixture.token_account(&fixture.outsider.pubkey());
    assert_eq!(fixture.token_state(newer).1, AccountState::Frozen);
    assert!(fixture.transfer(source, newer, 1_000).is_err());
    assert_eq!(fixture.token_state(source).0, 90_000);

    let close_ix = close_account(
        &TOKEN_2022_PROGRAM_ID,
        &fixture.mint.pubkey(),
        &fixture.issuer.pubkey(),
        &fixture.close.pubkey(),
        &[],
    )
    .unwrap();
    assert!(send(
        &mut fixture.svm,
        &fixture.issuer,
        &[close_ix.clone()],
        &[&fixture.close]
    )
    .is_err());
    assert!(fixture.svm.get_account(&fixture.mint.pubkey()).is_some());

    expect_success(send(
        &mut fixture.svm,
        &fixture.issuer,
        &[fee_instruction::harvest_withheld_tokens_to_mint(
            &TOKEN_2022_PROGRAM_ID,
            &fixture.mint.pubkey(),
            &[&destination],
        )
        .unwrap()],
        &[],
    ));
    assert_eq!(fixture.token_state(destination).2, 0);
    expect_success(send(
        &mut fixture.svm,
        &fixture.issuer,
        &[fee_instruction::withdraw_withheld_tokens_from_mint(
            &TOKEN_2022_PROGRAM_ID,
            &fixture.mint.pubkey(),
            &treasury,
            &fixture.withdraw.pubkey(),
            &[],
        )
        .unwrap()],
        &[&fixture.withdraw],
    ));
    assert_eq!(fixture.token_state(treasury).0, 250);

    for (account, owner, amount) in [
        (source, &fixture.issuer, 90_000),
        (destination, &fixture.outsider, 9_750),
        (treasury, &fixture.withdraw, 250),
    ] {
        let extra_signers: Vec<&Keypair> = if owner.pubkey() == fixture.issuer.pubkey() {
            vec![]
        } else {
            vec![owner]
        };
        expect_success(send(
            &mut fixture.svm,
            &fixture.issuer,
            &[burn(
                &TOKEN_2022_PROGRAM_ID,
                &account,
                &fixture.mint.pubkey(),
                &owner.pubkey(),
                &[],
                amount,
            )
            .unwrap()],
            &extra_signers,
        ));
    }
    let mint_account = fixture.svm.get_account(&fixture.mint.pubkey()).unwrap();
    let mint = StateWithExtensions::<MintState>::unpack(&mint_account.data).unwrap();
    assert_eq!(mint.base.supply, 0);
    expect_success(send(
        &mut fixture.svm,
        &fixture.issuer,
        &[close_ix],
        &[&fixture.close],
    ));
    assert!(fixture.svm.get_account(&fixture.mint.pubkey()).is_none());
}

#[test]
fn fee_uses_current_epoch_and_cap() {
    let mut fixture = Fixture::new();
    fixture.create_base_mint();
    let source = fixture.token_account(&fixture.issuer.pubkey());
    let destination = fixture.token_account(&fixture.outsider.pubkey());
    let freeze = fixture.freeze.insecure_clone();
    expect_success(fixture.thaw(source, &freeze));
    expect_success(fixture.thaw(destination, &freeze));
    expect_success(send(
        &mut fixture.svm,
        &fixture.issuer,
        &[mint_to(
            &TOKEN_2022_PROGRAM_ID,
            &fixture.mint.pubkey(),
            &source,
            &fixture.issuer.pubkey(),
            &[],
            10_000,
        )
        .unwrap()],
        &[],
    ));
    let fee_config = fixture.fee_config.insecure_clone();
    expect_success(send(
        &mut fixture.svm,
        &fixture.issuer,
        &[fee_instruction::set_transfer_fee(
            &TOKEN_2022_PROGRAM_ID,
            &fixture.mint.pubkey(),
            &fee_config.pubkey(),
            &[],
            1_000,
            40,
        )
        .unwrap()],
        &[&fee_config],
    ));
    let mint_data = fixture.svm.get_account(&fixture.mint.pubkey()).unwrap();
    let mint = StateWithExtensions::<MintState>::unpack(&mint_data.data).unwrap();
    let config = mint.get_extension::<TransferFeeConfig>().unwrap();
    let current_epoch = fixture.svm.get_sysvar::<Clock>().epoch;
    let active_fee = config.calculate_epoch_fee(current_epoch, 1_000).unwrap();
    let next_epoch = u64::from(config.newer_transfer_fee.epoch);
    assert!(next_epoch > current_epoch);
    assert_eq!(active_fee, 25);
    assert_eq!(config.calculate_epoch_fee(next_epoch, 1_000), Some(40));
    drop(mint_data);
    expect_success(fixture.transfer(source, destination, 1_000));
    assert_eq!(
        fixture.token_state(destination),
        (975, AccountState::Initialized, 25)
    );
    let mut clock = fixture.svm.get_sysvar::<Clock>();
    clock.epoch = next_epoch;
    fixture.svm.set_sysvar(&clock);
    expect_success(fixture.transfer(source, destination, 1_000));
    assert_eq!(fixture.token_state(source).0, 8_000);
    assert_eq!(
        fixture.token_state(destination),
        (1_935, AccountState::Initialized, 65)
    );
}
