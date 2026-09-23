use anchor_lang::{prelude::*, solana_program::program::invoke};
use anchor_spl::token_interface::{spl_token_2022, TokenInterface};
use spl_token_2022::{
    extension::{
        confidential_transfer::instruction as confidential_instruction,
        confidential_transfer_fee::instruction as confidential_fee_instruction,
        default_account_state::instruction as default_state_instruction,
        metadata_pointer::instruction as metadata_pointer_instruction,
        transfer_fee::{instruction as transfer_fee_instruction, TransferFeeConfig},
        BaseStateWithExtensions, ExtensionType, StateWithExtensions,
    },
    instruction as token_instruction,
    state::{Account as TokenAccountState, AccountState, Mint as MintState},
};
use spl_token_metadata_interface::{instruction as metadata_instruction, state::TokenMetadata};
use crate::MintError;

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct RemittanceMintArgs {
    pub decimals: u8,
    pub transfer_fee_basis_points: u16,
    pub maximum_fee: u64,
    pub freeze_authority: Pubkey,
    pub close_authority: Pubkey,
    pub fee_config_authority: Pubkey,
    pub withdraw_withheld_authority: Pubkey,
    pub name: String,
    pub symbol: String,
    pub uri: String,
}

pub struct RemittanceConfidentialArgs {
    pub confidential_authority: Pubkey,
    pub permanent_delegate: Pubkey,
    pub confidential_fee_withdrawal_elgamal_pubkey: [u8; 32],
}

#[derive(Accounts)]
pub struct CreateRemittanceMint<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(mut)]
    pub mint: Signer<'info>,
    #[account(address = spl_token_2022::ID)]
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ThawRemittanceAccount<'info> {
    /// CHECK: Token-2022 ownership is constrained, the account is parsed with StateWithExtensions, and the thaw CPI validates its mint and authority.
    #[account(mut, owner = token_program.key())]
    pub token_account: UncheckedAccount<'info>,
    /// CHECK: Token-2022 ownership is constrained, the mint is parsed with StateWithExtensions, and the thaw CPI validates it.
    #[account(owner = token_program.key())]
    pub mint: UncheckedAccount<'info>,
    pub freeze_authority: Signer<'info>,
    #[account(address = spl_token_2022::ID)]
    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct TransferWithFee<'info> {
    /// CHECK: Token-2022 ownership is constrained, the token account is parsed with StateWithExtensions, and its owner and mint are checked in the handler.
    #[account(mut, owner = token_program.key())]
    pub source: UncheckedAccount<'info>,
    /// CHECK: Token-2022 ownership is constrained and the mint and fee extension are parsed with StateWithExtensions in the handler.
    #[account(owner = token_program.key())]
    pub mint: UncheckedAccount<'info>,
    /// CHECK: Token-2022 ownership is constrained, the token account is parsed with StateWithExtensions, and its mint is checked in the handler.
    #[account(mut, owner = token_program.key())]
    pub destination: UncheckedAccount<'info>,
    pub owner: Signer<'info>,
    #[account(address = spl_token_2022::ID)]
    pub token_program: Interface<'info, TokenInterface>,
}

pub fn create_mint(
    ctx: Context<CreateRemittanceMint>,
    args: RemittanceMintArgs,
    confidential: Option<RemittanceConfidentialArgs>,
) -> Result<()> {
    let mint = ctx.accounts.mint.key();
    let payer = ctx.accounts.payer.key();
    let token_program = ctx.accounts.token_program.key();
    let mut extensions = vec![
        ExtensionType::TransferFeeConfig,
        ExtensionType::MetadataPointer,
        ExtensionType::DefaultAccountState,
        ExtensionType::MintCloseAuthority,
    ];
    if confidential.is_some() {
        extensions.extend_from_slice(&[
            ExtensionType::PermanentDelegate,
            ExtensionType::ConfidentialTransferMint,
            ExtensionType::ConfidentialTransferFeeConfig,
        ]);
    }
    let fixed_space = ExtensionType::try_calculate_account_len::<MintState>(&extensions)?;
    let metadata = TokenMetadata {
        update_authority: Some(payer).try_into()?,
        mint,
        name: args.name.clone(),
        symbol: args.symbol.clone(),
        uri: args.uri.clone(),
        additional_metadata: Vec::new(),
    };
    let final_space = fixed_space
        .checked_add(metadata.tlv_size_of()?)
        .ok_or(MintError::InvalidMetadataSize)?;
    let rent = Rent::get()?.minimum_balance(final_space);

    anchor_lang::system_program::create_account(
        CpiContext::new(
            ctx.accounts.system_program.key(),
            anchor_lang::system_program::CreateAccount {
                from: ctx.accounts.payer.to_account_info(),
                to: ctx.accounts.mint.to_account_info(),
            },
        ),
        rent,
        fixed_space as u64,
        &token_program,
    )?;

    let mint_info = ctx.accounts.mint.to_account_info();
    let payer_info = ctx.accounts.payer.to_account_info();
    let program_info = ctx.accounts.token_program.to_account_info();
    let mint_accounts = &[mint_info.clone(), program_info.clone()];

    invoke(
        &token_instruction::initialize_mint_close_authority(
            &token_program,
            &mint,
            Some(&args.close_authority),
        )?,
        mint_accounts,
    )?;
    invoke(
        &transfer_fee_instruction::initialize_transfer_fee_config(
            &token_program,
            &mint,
            Some(&args.fee_config_authority),
            Some(&args.withdraw_withheld_authority),
            args.transfer_fee_basis_points,
            args.maximum_fee,
        )?,
        mint_accounts,
    )?;
    invoke(
        &metadata_pointer_instruction::initialize(&token_program, &mint, Some(payer), Some(mint))?,
        mint_accounts,
    )?;
    invoke(
        &default_state_instruction::initialize_default_account_state(
            &token_program,
            &mint,
            &AccountState::Frozen,
        )?,
        mint_accounts,
    )?;

    if let Some(extra) = confidential {
        invoke(
            &token_instruction::initialize_permanent_delegate(
                &token_program,
                &mint,
                &extra.permanent_delegate,
            )?,
            mint_accounts,
        )?;
        invoke(
            &confidential_instruction::initialize_mint(
                &token_program,
                &mint,
                Some(extra.confidential_authority),
                false,
                None,
            )?,
            mint_accounts,
        )?;
        invoke(
            &confidential_fee_instruction::initialize_confidential_transfer_fee_config(
                &token_program,
                &mint,
                Some(args.withdraw_withheld_authority),
                &extra.confidential_fee_withdrawal_elgamal_pubkey.into(),
            )?,
            mint_accounts,
        )?;
    }

    invoke(
        &token_instruction::initialize_mint2(
            &token_program,
            &mint,
            &payer,
            Some(&args.freeze_authority),
            args.decimals,
        )?,
        mint_accounts,
    )?;

    invoke(
        &metadata_instruction::initialize(
            &token_program,
            &mint,
            &payer,
            &mint,
            &payer,
            args.name,
            args.symbol,
            args.uri,
        ),
        &[mint_info, payer_info, program_info],
    )?;

    Ok(())
}

pub fn thaw_account(ctx: Context<ThawRemittanceAccount>) -> Result<()> {
    {
        let data = ctx.accounts.mint.try_borrow_data()?;
        StateWithExtensions::<MintState>::unpack(&data)?;
    }
    {
        let data = ctx.accounts.token_account.try_borrow_data()?;
        let state = StateWithExtensions::<TokenAccountState>::unpack(&data)?;
        require_keys_eq!(
            state.base.mint,
            ctx.accounts.mint.key(),
            MintError::WrongMint
        );
        require!(
            state.base.state == AccountState::Frozen,
            MintError::AccountNotFrozen
        );
    }
    let ix = token_instruction::thaw_account(
        &ctx.accounts.token_program.key(),
        &ctx.accounts.token_account.key(),
        &ctx.accounts.mint.key(),
        &ctx.accounts.freeze_authority.key(),
        &[],
    )?;
    invoke(
        &ix,
        &[
            ctx.accounts.token_account.to_account_info(),
            ctx.accounts.mint.to_account_info(),
            ctx.accounts.freeze_authority.to_account_info(),
            ctx.accounts.token_program.to_account_info(),
        ],
    )?;
    Ok(())
}

pub fn transfer_fee_checked(ctx: Context<TransferWithFee>, amount: u64) -> Result<()> {
    require!(amount > 0, MintError::ZeroTransfer);
    {
        let source_data = ctx.accounts.source.try_borrow_data()?;
        let source = StateWithExtensions::<TokenAccountState>::unpack(&source_data)?;
        require_keys_eq!(
            source.base.owner,
            ctx.accounts.owner.key(),
            MintError::WrongOwner
        );
        require_keys_eq!(
            source.base.mint,
            ctx.accounts.mint.key(),
            MintError::WrongMint
        );
    }
    {
        let destination_data = ctx.accounts.destination.try_borrow_data()?;
        let destination = StateWithExtensions::<TokenAccountState>::unpack(&destination_data)?;
        require_keys_eq!(
            destination.base.mint,
            ctx.accounts.mint.key(),
            MintError::WrongMint
        );
    }
    let (decimals, fee) = {
        let mint_data = ctx.accounts.mint.try_borrow_data()?;
        let mint = StateWithExtensions::<MintState>::unpack(&mint_data)?;
        let config = mint
            .get_extension::<TransferFeeConfig>()
            .map_err(|_| MintError::MissingTransferFee)?;
        let fee = config
            .calculate_epoch_fee(Clock::get()?.epoch, amount)
            .ok_or(MintError::FeeCalculationFailed)?;
        (mint.base.decimals, fee)
    };
    let ix = transfer_fee_instruction::transfer_checked_with_fee(
        &ctx.accounts.token_program.key(),
        &ctx.accounts.source.key(),
        &ctx.accounts.mint.key(),
        &ctx.accounts.destination.key(),
        &ctx.accounts.owner.key(),
        &[],
        amount,
        decimals,
        fee,
    )?;
    invoke(
        &ix,
        &[
            ctx.accounts.source.to_account_info(),
            ctx.accounts.mint.to_account_info(),
            ctx.accounts.destination.to_account_info(),
            ctx.accounts.owner.to_account_info(),
            ctx.accounts.token_program.to_account_info(),
        ],
    )?;
    Ok(())
}
