use std::cell::RefMut;
use std::collections::VecDeque;
use std::ops::Deref;

use crate::error::ErrorCode;
use crate::libraries::tick_math;
use crate::swap::swap_internal;
use crate::util::*;
use crate::{states::*, util};
use anchor_lang::prelude::*;
use anchor_spl::token::Token;
use anchor_spl::token_2022;
use anchor_spl::token_interface::{Mint, Token2022, TokenAccount};

/// Memo msg for swap
pub const SWAP_MEMO_MSG: &'static [u8] = b"raydium_swap";
#[derive(Accounts)]
pub struct SwapSingleV2<'info> {
    /// The user performing the swap
    #[account(mut)]
    pub payer: Signer<'info>,

    /// The factory state to read protocol fees
    #[account(address = pool_state.load()?.amm_config)]
    pub amm_config: Box<Account<'info, AmmConfig>>,

    /// The program account of the pool in which the swap will be performed
    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,

    /// The user token account for input token
    #[account(mut)]
    pub input_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The user token account for output token
    #[account(mut)]
    pub output_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The vault token account for output token
    #[account(
        mut,
       // mint::decimals = input_vault_mint.decimals,
        mint::authority = pool_state,
    )]
    pub input_leveraged_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The vault token account for output token
    #[account(
        mut,
      //  mint::decimals = output_vault_mint.decimals,
        mint::authority = pool_state)]
    pub output_leveraged_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The user token account for input token
    #[account(mut)]
    pub input_leveraged_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The user token account for output token
    #[account(mut)]
    pub output_leveraged_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The vault token account for input token
    #[account(mut)]
    pub input_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The vault token account for output token
    #[account(mut)]
    pub output_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The program account for the most recent oracle observation
    #[account(mut, address = pool_state.load()?.observation_key)]
    pub observation_state: AccountLoader<'info, ObservationState>,

    /// SPL program for token transfers
    pub token_program: Program<'info, Token>,

    /// SPL program 2022 for token transfers
    pub token_program_2022: Program<'info, Token2022>,

    /// CHECK:
    #[account(
        address = spl_memo::id()
    )]
    pub memo_program: UncheckedAccount<'info>,

    /// The mint of token vault 0
    #[account(
    )]
    pub input_vault_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The mint of token vault 1
    #[account(
    )]
    pub output_vault_mint: Box<InterfaceAccount<'info, Mint>>,
    pub other_pool_state: AccountLoader<'info, PoolState>,
    pub system_program: Program<'info, System>,

    // remaining accounts
    // tickarray_bitmap_extension: must add account if need regardless the sequence
    // tick_array_account_1
    // tick_array_account_2
    // tick_array_account_...
}

/// Performs a single exact input/output swap
/// if is_base_input = true, return vaule is the max_amount_out, otherwise is min_amount_in
pub fn exact_internal_v2<'c: 'info, 'info>(
    ctx: &mut SwapSingleV2<'info>,
    remaining_accounts: &'c [AccountInfo<'info>],
    amount_specified: u64,
    sqrt_price_limit_x64: u128,
    is_base_input: bool,
) -> Result<u64> {
    // invoke_memo_instruction(SWAP_MEMO_MSG, ctx.memo_program.to_account_info())?;

    let block_timestamp = solana_program::clock::Clock::get()?.unix_timestamp as u64;

    let amount_0;
    let amount_1;
    let zero_for_one;
    let swap_price_before;

    let input_balance_before = ctx.input_token_account.amount;
    let output_balance_before = ctx.output_token_account.amount;

    // calculate specified amount because the amount includes thransfer_fee as input and without thransfer_fee as output
    let amount_specified = if is_base_input {
        let transfer_fee =
            util::get_transfer_fee(ctx.input_vault_mint.clone(), amount_specified).unwrap();
        amount_specified - transfer_fee
    } else {
        let transfer_fee =
            util::get_transfer_inverse_fee(ctx.output_vault_mint.clone(), amount_specified)
                .unwrap();
        amount_specified + transfer_fee
    };

    {
        swap_price_before = ctx.pool_state.load()?.sqrt_price_x64;
        let pool_state = &mut ctx.pool_state.load_mut()?;
        if pool_state.leveraged_mint_0 == None 
            && pool_state.leveraged_mint_1 == None
        {
           pool_state.leveraged_mint_0 = Some(ctx.input_leveraged_mint.key());
              pool_state.leveraged_mint_1 = Some(ctx.output_leveraged_mint.key());
        }
        if pool_state.leveraged_mint_0.unwrap() != ctx.input_leveraged_mint.key()
            && pool_state.leveraged_mint_1.unwrap() != ctx.output_leveraged_mint.key()
        {
            return Err(ErrorCode::NotApproved.into());
        }
        zero_for_one = ctx.input_vault_mint.key() == pool_state.token_mint_0;

        require_gt!(block_timestamp, pool_state.open_time);

        let mut tickarray_bitmap_extension = None;
        let tick_array_states = &mut VecDeque::new();

        let tick_array_bitmap_extension_key = TickArrayBitmapExtension::key(pool_state.key());
        for account_info in remaining_accounts.into_iter() {
            if account_info.key().eq(&tick_array_bitmap_extension_key) {
                tickarray_bitmap_extension = Some(
                    *(AccountLoader::<TickArrayBitmapExtension>::try_from(account_info)?
                        .load()?
                        .deref()),
                );
                continue;
            }
            tick_array_states.push_back(AccountLoad::load_data_mut(account_info)?);
        }

        (amount_0, amount_1) = swap_internal(
            &ctx.amm_config,
            pool_state,
            tick_array_states,
            &mut ctx.observation_state.load_mut()?,
            &tickarray_bitmap_extension,
            amount_specified,
            if sqrt_price_limit_x64 == 0 {
                if zero_for_one {
                    tick_math::MIN_SQRT_PRICE_X64 + 1
                } else {
                    tick_math::MAX_SQRT_PRICE_X64 - 1
                }
            } else {
                sqrt_price_limit_x64
            },
            zero_for_one,
            is_base_input,
            oracle::block_timestamp(),
        )?;

        #[cfg(feature = "enable-log")]
        msg!(
            "exact_swap_internal, is_base_input:{}, amount_0: {}, amount_1: {}",
            is_base_input,
            amount_0,
            amount_1
        );
        require!(
            amount_0 != 0 && amount_1 != 0,
            ErrorCode::TooSmallInputOrOutputAmount
        );
    }
    let (token_account_0, token_account_1, vault_0, vault_1, leveraged_mint_0, leveraged_account_0, leveraged_mint_1, leveraged_account_1, vault_0_mint, vault_1_mint) =
        if zero_for_one {
            (
                ctx.input_token_account.clone(),
                ctx.output_token_account.clone(),
                ctx.input_vault.clone(),
                ctx.output_vault.clone(),
                ctx.input_leveraged_mint.clone(),
                ctx.input_leveraged_account.clone(),
                ctx.output_leveraged_mint.clone(),
                ctx.output_leveraged_account.clone(),
                ctx.input_vault_mint.clone(),
                ctx.output_vault_mint.clone(),
            )
        } else {
            (
                ctx.output_token_account.clone(),
                ctx.input_token_account.clone(),
                ctx.output_vault.clone(),
                ctx.input_vault.clone(),
                ctx.output_leveraged_mint.clone(),
                ctx.output_leveraged_account.clone(),
                ctx.input_leveraged_mint.clone(),
                ctx.input_leveraged_account.clone(),
                ctx.output_vault_mint.clone(),
                ctx.input_vault_mint.clone(),
            )
        };

    // user or pool real amount delta without tranfer fee
    let amount_0_without_fee;
    let amount_1_without_fee;
    // the transfer fee amount charged by withheld_amount
    let transfer_fee_0;
    let transfer_fee_1;
    if zero_for_one {
        transfer_fee_0 = util::get_transfer_inverse_fee(vault_0_mint.clone(), amount_0).unwrap();
        transfer_fee_1 = util::get_transfer_fee(vault_1_mint.clone(), amount_1).unwrap();

        amount_0_without_fee = amount_0;
        amount_1_without_fee = amount_1.checked_sub(transfer_fee_1).unwrap();
        let (transfer_amount_0, transfer_amount_1) = (amount_0 + transfer_fee_0, amount_1);
        #[cfg(feature = "enable-log")]
        msg!(
            "amount_0:{}, transfer_fee_0:{}, amount_1:{}, transfer_fee_1:{}",
            amount_0,
            transfer_fee_0,
            amount_1,
            transfer_fee_1
        );
        //  x -> y, deposit x token from user to pool vault.
        transfer_from_user_to_pool_vault(
            &ctx.payer,
            &token_account_0,
            &vault_0,
            Some(vault_0_mint),
            &ctx.token_program,
            Some(ctx.token_program_2022.to_account_info()),
            transfer_amount_0,
        )?;
        // x -> y，transfer y token from pool vault to user.
        mint_leveraged_tokens_to_user(
            &ctx.pool_state,
            &leveraged_account_1,
            &leveraged_mint_1,
            &ctx.token_program,
            transfer_amount_1,
        )?;
    } else {
        transfer_fee_0 = util::get_transfer_fee(vault_0_mint.clone(), amount_0).unwrap();
        transfer_fee_1 = util::get_transfer_inverse_fee(vault_1_mint.clone(), amount_1).unwrap();

        amount_0_without_fee = amount_0.checked_sub(transfer_fee_0).unwrap();
        amount_1_without_fee = amount_1;
        let (transfer_amount_0, transfer_amount_1) = (amount_0, amount_1 + transfer_fee_1);

        msg!("amount_0:{}, transfer_fee_0:{}", amount_0, transfer_fee_0);
        msg!("amount_1:{}, transfer_fee_1:{}", amount_1, transfer_fee_1);
        transfer_from_user_to_pool_vault(
            &ctx.payer,
            &token_account_1,
            &vault_1,
            Some(vault_1_mint),
            &ctx.token_program,
            Some(ctx.token_program_2022.to_account_info()),
            transfer_amount_1,
        )?;
        mint_leveraged_tokens_to_user(
            &ctx.pool_state,
            &leveraged_account_0,
            &leveraged_mint_0,
            &ctx.token_program,
            transfer_amount_0
        )?;
    }
    ctx.output_token_account.reload()?;
    ctx.input_token_account.reload()?;

    let pool_state = ctx.pool_state.load()?;
    emit!(SwapEvent {
        pool_state: pool_state.key(),
        sender: ctx.payer.key(),
        token_account_0: token_account_0.key(),
        token_account_1: token_account_1.key(),
        amount_0: amount_0_without_fee,
        transfer_fee_0,
        amount_1: amount_1_without_fee,
        transfer_fee_1,
        zero_for_one,
        sqrt_price_x64: pool_state.sqrt_price_x64,
        liquidity: pool_state.liquidity,
        tick: pool_state.tick_current
    });
    if zero_for_one {
        require_gt!(swap_price_before, pool_state.sqrt_price_x64);
    } else {
        require_gt!(pool_state.sqrt_price_x64, swap_price_before);
    }

    if is_base_input {
        Ok(ctx
            .output_token_account
            .amount
            .checked_sub(output_balance_before)
            .unwrap())
    } else {
        Ok(input_balance_before
            .checked_sub(ctx.input_token_account.amount)
            .unwrap())
    }
}

pub fn swap_v2<'a, 'b, 'c: 'info, 'info>(
    ctx: Context<'a, 'b, 'c, 'info, SwapSingleV2<'info>>,
    amount: u64,
    other_amount_threshold: u64,
    sqrt_price_limit_x64: u128,
    is_base_input: bool,
) -> Result<()> {
    let amount_result = exact_internal_v2(
        ctx.accounts,
        ctx.remaining_accounts,
        amount,
        sqrt_price_limit_x64,
        is_base_input,
    )?;
    if is_base_input {
        require_gte!(
            amount_result,
            other_amount_threshold,
            ErrorCode::TooLittleOutputReceived
        );
    } else {
        require_gte!(
            other_amount_threshold,
            amount_result,
            ErrorCode::TooMuchInputPaid
        );
    }

    Ok(())
}

use anchor_spl::token::{self, Burn, Transfer};
use num_integer::Roots;
use solana_program::program::invoke_signed;
use solana_program::program_pack::Pack;

#[derive(Accounts)]
pub struct UnswapSingleV2<'info> {
    /// The user performing the unswap
    #[account(mut)]
    pub user: Signer<'info>,

    /// The leveraged token account from which tokens will be burned
    #[account(mut)]
    pub leveraged_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The mint of the leveraged token
    #[account(mut)]
    pub leveraged_token_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The user's account to which the collateral will be returned
    #[account(mut)]
    pub collateral_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The pool state to update after burning leveraged tokens
    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,
    #[account(mut)]
    pub other_pool_sate: AccountLoader<'info, PoolState>,
    #[account(mut)]
    pub pool_state_base_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut)]
    pub pool_state_quote_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut)]
    pub other_pool_state_base_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut)]
    pub other_pool_state_quote_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// The Token Mint 0 
    #[account(mut)]
    pub token_mint_0: Box<InterfaceAccount<'info, Mint>>,
    /// The Token Mint 1
    #[account(mut)]
    pub token_mint_1: Box<InterfaceAccount<'info, Mint>>,
    /// SPL Token program
    pub token_program: Program<'info, Token>,

    /// SPL Token program 2022

    pub token_program_2022: Program<'info, Token2022>,

    /// The account that holds the collateral within the pool
    #[account(mut)]
    pub pool_collateral_account: Box<InterfaceAccount<'info, TokenAccount>>,
}
pub fn unswap_v2<'a, 'b, 'c: 'info, 'info>(
    ctx: Context<'a, 'b, 'c, 'info, UnswapSingleV2<'info>>,
    amount_to_burn: u64,
) -> Result<()> {
    let mut pool_ratio: f64;
    let mut other_pool_ratio: f64;
    {
    // Burn the leveraged tokens from the user's account
    let burn_cpi_accounts = Burn {
        mint: ctx.accounts.leveraged_token_mint.to_account_info(),
        from: ctx.accounts.leveraged_token_account.to_account_info(),
        authority: ctx.accounts.user.to_account_info(),
    };
    let burn_cpi_program = ctx.accounts.token_program.to_account_info();
    let burn_cpi_ctx = CpiContext::new(burn_cpi_program, burn_cpi_accounts);
    token::burn(burn_cpi_ctx, amount_to_burn)?;
}

let mut accounts = vec![];
accounts.push(ctx.accounts.pool_state.to_account_info());
msg!("accounts: {:?}", accounts.len());
accounts.push(ctx.accounts.other_pool_sate.to_account_info());
msg!("accounts: {:?}", accounts.len());
accounts.push(ctx.accounts.pool_state_base_vault.to_account_info());
msg!("accounts: {:?}", accounts.len());
accounts.push(ctx.accounts.pool_state_quote_vault.to_account_info());
msg!("accounts: {:?}", accounts.len());
accounts.push(ctx.accounts.other_pool_state_base_vault.to_account_info());
msg!("accounts: {:?}", accounts.len());
accounts.push(ctx.accounts.other_pool_state_quote_vault.to_account_info());
msg!("accounts: {:?}", accounts.len());


    // implement the logic to rebalance the pools based on the amount of tokens being burned
    // This should consider the pool's leverage, the amount of tokens being burned, and any fees or penalties
    // The pool's state should be updated to reflect the new balances of the base and quote vaults
    // The other pool's state should also be updated to reflect the new balances of the base and quote vaults

    let pool_state = ctx.accounts.pool_state.load()?;

    let other_pool_state = ctx.accounts.other_pool_sate.load()?;

    // Calculate the amount of collateral to return based on the rebalanced pool state and power leverage
    // This function needs to be defined according to your protocol's specific leverage calculations
    let leverage = pool_state.leverage.into_iter().next().unwrap();
    msg!("leverage: {}", leverage);
    {


    let collateral_to_return = calculate_collateral_to_return(&pool_state, &other_pool_state, amount_to_burn)?;
    // Transfer the collateral from the pool's collateral account back to the user's collateral account
    let transfer_cpi_accounts = Transfer {
        from: ctx.accounts.pool_collateral_account.to_account_info(),
        to: ctx.accounts.collateral_account.to_account_info(),
        authority: ctx.accounts.pool_state.to_account_info(),
    };
    let signer = &[&pool_state.seeds()[..]];
    let transfer_cpi_program = ctx.accounts.token_program.to_account_info();
    let transfer_cpi_ctx = CpiContext::new_with_signer(transfer_cpi_program, transfer_cpi_accounts, signer);
    token::transfer(transfer_cpi_ctx, collateral_to_return as u64 / 1_000_000_000)?;
}
{
    let price = pool_state.sqrt_price_x64 as f64;
    let other_price = other_pool_state.sqrt_price_x64 as f64;
    let which_pool_seeds = if price > other_price {
        pool_state.seeds()
    } else {
        other_pool_state.seeds()
    };
    let last_price_on_rebalance = pool_state.price_on_last_rebalance as f64;
    let other_last_price_on_rebalance = other_pool_state.price_on_last_rebalance as f64;
     pool_ratio = calculate_adjustment_ratio(
        price, 
        last_price_on_rebalance,    
        leverage as u64);
             other_pool_ratio = calculate_adjustment_ratio(
        other_price,
        other_last_price_on_rebalance,
        leverage as u64);

    let pool_state_key = pool_state.key();
    let other_pool_state_key = other_pool_state.key();
    let leverage = pool_state.leverage.into_iter().next().unwrap();

   
  
    let base_vault = ctx.accounts.pool_state_base_vault.to_account_info();
    let other_base_vault = ctx.accounts.other_pool_state_base_vault.to_account_info();
    let quote_vault = ctx.accounts.pool_state_quote_vault.to_account_info();
    let other_quote_vault = ctx.accounts.other_pool_state_quote_vault.to_account_info();
    let amount_to_transfer = if price > other_price {
        let hm = spl_token::state::Account::unpack(&base_vault.clone().data.borrow())?;
        
        ((1_f64 - pool_ratio as f64) * hm.amount as f64) as u128 
    } else {
        let hm = spl_token::state::Account::unpack(&quote_vault.clone().data.borrow())?;
       ((1_f64 - pool_ratio as f64) * hm.amount as f64) as u128 
    };
    
    

    

    msg!("pool_ratio: {}", pool_ratio);
    // figure out the amount of base and quote tokens to transfer based on power leverage
    // first find out which is higher price
    msg!("amount_to_transfer: {}", amount_to_transfer);
        let old_token_program = if price > other_price {
            if base_vault.owner == ctx.accounts.token_program_2022.key {
                false
            } else {
                true
            }
        } else {
            if other_base_vault.owner == ctx.accounts.token_program_2022.key {
                false
            } else {
                true 
            }
        };
        msg!("old_token_program: {}", old_token_program);
        msg!("which_pool_seeds: {}", which_pool_seeds.len());
        match old_token_program {
            true => {
                let ix = 
                    spl_token::instruction::transfer(
                        &ctx.accounts.token_program.key(),
                        &if price > other_price {
                            base_vault.key()
                        } else {
                            other_quote_vault.key()
                        },
                        &if price > other_price {
                            other_quote_vault.key()
                        } else {
                            base_vault.key()
                        },
                        &if price > other_price {
                            pool_state_key 
                        } else {
                            other_pool_state_key
                        },
                        &[],
                        amount_to_transfer as u64
                    )?;
                msg!("ix: {:?}", ix);
                invoke_signed(
                    &ix,
                    &accounts,
                    &[&which_pool_seeds],
                )?;
            }
            false => {
                token_2022::transfer_checked(
                    CpiContext::new_with_signer(
                        ctx.accounts.token_program_2022.to_account_info(),
                        token_2022::TransferChecked {
                            from: base_vault,
                            to: other_base_vault.clone(),
                            authority: if price > other_price {
                                ctx.accounts.pool_state.to_account_info()
                            } else {
                                ctx.accounts.other_pool_sate.to_account_info()
                            },
                            mint: ctx.accounts.token_mint_0.to_account_info()
                        },
                        &[&which_pool_seeds],
                    ),
                    amount_to_transfer as u64,
                    ctx.accounts.token_mint_0.decimals,
                )?;
            }
        }
    // repeat for quote
    let amount_to_transfer = if price < other_price {
        let hm = spl_token::state::Account::unpack(&other_base_vault.clone().data.borrow())?;
        
        ((1_f64 - other_pool_ratio as f64) * hm.amount as f64) as u128 
    } else {
        let hm = spl_token::state::Account::unpack(&other_quote_vault.clone().data.borrow())?;
       ((1_f64 - other_pool_ratio as f64) * hm.amount as f64) as u128
    };
    
    
    msg!("amount_to_transfer: {}", amount_to_transfer);

    msg!("pool_ratio: {}", pool_ratio);
    // figure out the amount of base and quote tokens to transfer based on power leverage
    // first find out which is higher price
  
    let quote_vault = ctx.accounts.pool_state_quote_vault.to_account_info();
    let other_quote_vault = ctx.accounts.other_pool_state_quote_vault.to_account_info();
    let base_vault = ctx.accounts.pool_state_base_vault.to_account_info();
    let other_base_vault = ctx.accounts.other_pool_state_base_vault.to_account_info();
    msg!("amount_to_transfer: {}", amount_to_transfer);
        let old_token_program = if price < other_price {
            if quote_vault.owner == ctx.accounts.token_program_2022.key {
                false
            } else {
                true
            }
        } else {
            if other_quote_vault.owner == ctx.accounts.token_program_2022.key {
                false
            } else {
                true 
            }
        };
        msg!("old_token_program: {}", old_token_program);
       
        msg!("which_pool_seeds: {}", which_pool_seeds.len());
        match old_token_program {
            true => {
               
                let ix = 
                    spl_token::instruction::transfer(
                        &ctx.accounts.token_program.key(),
                        &if price > other_price {
                            quote_vault.key()
                        } else {
                            other_base_vault.key()
                        },
                        &if price > other_price {
                            other_base_vault.key()
                        } else {
                            quote_vault.key()
                        },
                        &if price > other_price {
                            pool_state_key 
                        } else {
                            other_pool_state_key 
                        },
                        &[],
                        amount_to_transfer as u64
                    )?;
                msg!("ix: {:?}", ix);
                
                invoke_signed(
                    &ix,
                    &accounts,
                    &[&which_pool_seeds],
                )?;
            }
            false => {
                token_2022::transfer_checked(
                    CpiContext::new_with_signer(
                        ctx.accounts.token_program_2022.to_account_info(),
                        token_2022::TransferChecked {
                            from: quote_vault,
                            to: other_quote_vault,
                            authority: if price > other_price {
                                ctx.accounts.pool_state.to_account_info()
                            } else {
                                ctx.accounts.other_pool_sate.to_account_info()
                            },
                            mint: ctx.accounts.token_mint_1.to_account_info()
                        },
                        &[&which_pool_seeds],
                    ),
                    amount_to_transfer as u64,
                    ctx.accounts.token_mint_1.decimals,
                )?;
            }
    }
}
drop(pool_state);
drop(other_pool_state);
{
let mut pool_state = ctx.accounts.pool_state.load_mut()?;
let mut other_pool_state = ctx.accounts.other_pool_sate.load_mut()?;

    let price = pool_state.sqrt_price_x64;
    let other_price = other_pool_state.sqrt_price_x64;
    let leverage = pool_state.leverage.into_iter().next().unwrap();
   
    pool_state.price_on_last_rebalance = price;
    other_pool_state.price_on_last_rebalance = other_price;
    pool_state.curr_ratio = pool_ratio as u128;
    other_pool_state.curr_ratio = other_pool_ratio as u128 ;
}

    Ok(())
}
fn calculate_adjustment_ratio(
    current_price: f64,
    last_rebalance_price: f64,
    leverage: u64,
) -> f64 {
    msg!("current_price: {}", current_price);
    msg!("last_rebalance_price: {}", last_rebalance_price);
    // Calculate the price movement ratio since the last rebalance
    let price_movement_ratio = if last_rebalance_price > 0.0 {
        current_price / last_rebalance_price
    } else {
        1.0
    };
    msg!("price_movement_ratio: {}", price_movement_ratio);

    // Adjust the ratio by the leverage factor
    // it is a sqrt_x64 
    let adjustment_ratio = price_movement_ratio.powf(leverage as f64);
    msg!("adjustment_ratio: {}", adjustment_ratio);
    
    // Return the adjustment ratio as a u128
    (adjustment_ratio )
}

// Implement this function based on your pool's specific logic for calculating the collateral to return
fn calculate_collateral_to_return(pool_state: &PoolState, other_pool_state: &PoolState, amount_to_burn: u64) -> Result<u64> {
    // logic for profit/loss
    // checks price of pool vs other pool
    // checks amount of tokens burned
    // returns amount of collateral to return
   
    let base_value: f64 = 1_000_000_000f64;

    let pool_ratio = if pool_state.curr_ratio == 0 {
        1_000_000_000
    } else {
        pool_state.curr_ratio
    };
    let ratio = (pool_ratio as f64 / base_value) as f64;
    let amount_to_return = (amount_to_burn as f64 / ratio) as u64;
    
        
    Ok(amount_to_return as u64)
}