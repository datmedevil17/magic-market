use anchor_lang::prelude::*;
use ephemeral_rollups_sdk::anchor::{commit, delegate, ephemeral};
use ephemeral_rollups_sdk::cpi::DelegateConfig;
use ephemeral_rollups_sdk::ephem::{commit_accounts, commit_and_undelegate_accounts};
use pyth_sdk_solana::state::SolanaPriceAccount;

declare_id!("D2CgiFkSd8yk5dZif9V7JSgUs9teAdrRcCYcZ2f53ivJ");

// ============================================================================
// Constants
// ============================================================================

pub const MARKET_SEED: &[u8] = b"market";
pub const POOL_SEED: &[u8] = b"pool";
pub const POSITION_SEED: &[u8] = b"position";
pub const VAULT_SEED: &[u8] = b"vault";
pub const LP_POSITION_SEED: &[u8] = b"lp_position";

pub const BASIS_POINTS: u64 = 10000;
pub const LP_FEE_BPS: u64 = 30; // 0.3% fee
pub const MIN_LIQUIDITY: u64 = 1000; // Minimum initial liquidity
pub const PRICE_DECIMALS: u64 = 1_000_000; // 6 decimal precision for prices
pub const SHARE_DECIMALS: u64 = 1_000_000; // 6 decimal shares
pub const MAX_TRADE_SIZE_BPS: u64 = 1000; // 10% of pool max per trade
pub const RESOLUTION_DELAY: i64 = 300; // 5 minutes after expiration
pub const MIN_SHARES_OUTPUT: u64 = 1000; // Minimum shares to prevent dust
pub const MAX_ORACLE_STALENESS: i64 = 300; // 5 minutes max staleness

// ============================================================================
// Program
// ============================================================================

#[ephemeral]
#[program]
pub mod prediction_market {
    use super::*;

    /// Create a new binary prediction market
    ///
    /// # Arguments
    /// * `market_id` - Unique identifier for the market
    /// * `strike_price` - The price threshold for resolution (scaled by 10^8 like Pyth)
    /// * `expiration` - Unix timestamp when the market expires
    /// * `max_confidence` - Maximum acceptable confidence interval for resolution
    /// * `description` - Short description of the market
    pub fn create_market(
        ctx: Context<CreateMarket>,
        market_id: [u8; 32],
        strike_price: i64,
        expiration: i64,
        max_confidence: u64,
        description: String,
    ) -> Result<()> {
        require!(
            expiration > Clock::get()?.unix_timestamp,
            MarketError::InvalidExpiration
        );
        require!(description.len() <= 128, MarketError::DescriptionTooLong);

        // Validate Pyth oracle account
        let price_feed = SolanaPriceAccount::account_info_to_feed(&ctx.accounts.pyth_price_account)
            .map_err(|_| MarketError::InvalidOraclePrice)?;

        // Verify oracle is publishing recent data
        price_feed
            .get_price_no_older_than(
                Clock::get()?.unix_timestamp,
                3600, // 1 hour max staleness for creation
            )
            .ok_or(MarketError::InvalidOraclePrice)?;

        let market = &mut ctx.accounts.market;
        market.authority = ctx.accounts.authority.key();
        market.market_id = market_id;
        market.strike_price = strike_price;
        market.expiration = expiration;
        market.pyth_price_account = ctx.accounts.pyth_price_account.key();
        market.max_confidence = max_confidence;
        market.status = MarketStatus::Active;
        market.outcome = None;
        market.resolution_price = None;
        market.resolution_timestamp = None;
        market.total_yes_shares = 0;
        market.total_no_shares = 0;
        market.description = description;
        market.bump = ctx.bumps.market;

        msg!(
            "Market {} created with strike price {}",
            hex::encode(market_id),
            strike_price
        );
        Ok(())
    }

    /// Initialize the liquidity pool for a market
    pub fn initialize_pool(ctx: Context<InitializePool>, initial_liquidity: u64) -> Result<()> {
        require!(
            initial_liquidity >= MIN_LIQUIDITY,
            MarketError::InsufficientLiquidity
        );

        // Transfer SOL to vault first
        let cpi_context = CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            anchor_lang::system_program::Transfer {
                from: ctx.accounts.authority.to_account_info(),
                to: ctx.accounts.vault.to_account_info(),
            },
        );
        anchor_lang::system_program::transfer(cpi_context, initial_liquidity * 2)?;

        // Initialize pool state
        let pool = &mut ctx.accounts.pool;
        pool.market = ctx.accounts.market.key();
        pool.yes_reserve = initial_liquidity;
        pool.no_reserve = initial_liquidity;
        pool.total_liquidity = initial_liquidity * 2;
        pool.total_fees_collected = 0;
        pool.lp_token_supply = initial_liquidity * 2; // Initial LP tokens = liquidity amount
        pool.bump = ctx.bumps.pool;

        // Create LP position for initial provider
        let lp_position = &mut ctx.accounts.lp_position;
        lp_position.user = ctx.accounts.authority.key();
        lp_position.pool = ctx.accounts.pool.key();
        lp_position.lp_tokens = initial_liquidity * 2;
        lp_position.bump = ctx.bumps.lp_position;

        msg!(
            "Pool initialized with {} lamports liquidity, {} LP tokens minted",
            initial_liquidity * 2,
            initial_liquidity * 2
        );
        Ok(())
    }

    /// Add liquidity to the pool
    pub fn add_liquidity(
        ctx: Context<ModifyLiquidity>,
        amount: u64,
        min_lp_tokens: u64,
    ) -> Result<()> {
        require!(
            ctx.accounts.market.status == MarketStatus::Active,
            MarketError::MarketNotActive
        );
        require!(amount > 0, MarketError::InvalidAmount);

        let pool = &mut ctx.accounts.pool;
        let total_liquidity = pool.total_liquidity;
        let total_lp_shares = pool.lp_token_supply;

        // Calculate LP tokens to mint
        // lp_tokens = amount * total_lp_shares / total_liquidity
        // Since we enforce 50/50 added value, we can just use total liquidity
        let lp_tokens_to_mint = if total_liquidity == 0 {
            amount
        } else {
            (amount as u128 * total_lp_shares as u128 / total_liquidity as u128) as u64
        };

        require!(
            lp_tokens_to_mint >= min_lp_tokens,
            MarketError::SlippageExceeded
        );

        // Transfer SOL to vault
        let cpi_context = CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            anchor_lang::system_program::Transfer {
                from: ctx.accounts.user.to_account_info(),
                to: ctx.accounts.vault.to_account_info(),
            },
        );
        anchor_lang::system_program::transfer(cpi_context, amount)?;

        // Update pool state - split 50/50
        let half_amount = amount / 2;
        pool.yes_reserve += half_amount;
        pool.no_reserve += amount - half_amount;
        pool.total_liquidity += amount;
        pool.lp_token_supply += lp_tokens_to_mint;

        // Update user position
        let lp_position = &mut ctx.accounts.lp_position;
        lp_position.lp_tokens += lp_tokens_to_mint;
        if lp_position.user == Pubkey::default() {
            lp_position.user = ctx.accounts.user.key();
            lp_position.pool = pool.key();
            lp_position.bump = ctx.bumps.lp_position;
        }

        msg!(
            "Added {} lamports liquidity, minted {} LP tokens",
            amount,
            lp_tokens_to_mint
        );
        Ok(())
    }

    /// Remove liquidity from the pool
    pub fn remove_liquidity(
        ctx: Context<ModifyLiquidity>,
        lp_tokens: u64,
        min_amount_out: u64,
    ) -> Result<()> {
        require!(lp_tokens > 0, MarketError::InvalidAmount);

        let lp_position = &mut ctx.accounts.lp_position;
        require!(
            lp_position.lp_tokens >= lp_tokens,
            MarketError::InsufficientShares
        );

        let pool = &mut ctx.accounts.pool;
        let total_liquidity = pool.total_liquidity;
        let total_lp_shares = pool.lp_token_supply;

        require!(total_lp_shares > 0, MarketError::InsufficientLiquidity);

        // Calculate amount to return
        // amount = lp_tokens * total_liquidity / total_lp_shares
        let amount_out =
            (lp_tokens as u128 * total_liquidity as u128 / total_lp_shares as u128) as u64;

        require!(amount_out >= min_amount_out, MarketError::SlippageExceeded);

        // Check vault balance
        let vault_lamports = ctx.accounts.vault.lamports();
        require!(
            vault_lamports >= amount_out,
            MarketError::InsufficientVaultFunds
        );

        // Transfer SOL from vault to user
        let bump = ctx.bumps.vault;
        let bump_slice = &[bump];
        let market_key = ctx.accounts.market.key();
        let seeds = &[VAULT_SEED, market_key.as_ref(), bump_slice];
        let signer_seeds = &[&seeds[..]];

        let cpi_context = CpiContext::new_with_signer(
            ctx.accounts.system_program.to_account_info(),
            anchor_lang::system_program::Transfer {
                from: ctx.accounts.vault.to_account_info(),
                to: ctx.accounts.user.to_account_info(),
            },
            signer_seeds,
        );
        anchor_lang::system_program::transfer(cpi_context, amount_out)?;

        // Update pool state
        let half_amount = amount_out / 2;
        pool.yes_reserve = pool.yes_reserve.saturating_sub(half_amount);
        pool.no_reserve = pool.no_reserve.saturating_sub(amount_out - half_amount);
        pool.total_liquidity = pool.total_liquidity.saturating_sub(amount_out);
        pool.lp_token_supply = pool.lp_token_supply.saturating_sub(lp_tokens);

        // Update user position
        lp_position.lp_tokens -= lp_tokens;

        msg!(
            "Removed liquidity: burned {} LP tokens for {} lamports",
            lp_tokens,
            amount_out
        );
        Ok(())
    }

    /// Buy YES or NO shares using the AMM
    /// This instruction is designed to run on ephemeral rollups for instant execution
    pub fn buy_shares(
        ctx: Context<Trade>,
        side: Outcome,
        amount_in: u64,
        min_shares_out: u64,
    ) -> Result<()> {
        require!(
            ctx.accounts.market.status == MarketStatus::Active,
            MarketError::MarketNotActive
        );
        require!(amount_in > 0, MarketError::InvalidAmount);

        let pool = &mut ctx.accounts.pool;

        // Check for max trade size (10% of total liquidity)
        require!(
            amount_in <= pool.total_liquidity * MAX_TRADE_SIZE_BPS / BASIS_POINTS,
            MarketError::TradeExceedsMaxSize
        );
        require!(
            pool.yes_reserve > 0 && pool.no_reserve > 0,
            MarketError::PoolNotInitialized
        );

        // Calculate fee
        let fee = amount_in * LP_FEE_BPS / BASIS_POINTS;
        let amount_after_fee = amount_in - fee;

        // Calculate shares using constant product formula
        // For buying YES: shares_out = yes_reserve - (k / (no_reserve + amount))
        let (reserve_in, reserve_out) = match side {
            Outcome::Yes => (pool.no_reserve, pool.yes_reserve),
            Outcome::No => (pool.yes_reserve, pool.no_reserve),
        };

        let k = reserve_in as u128 * reserve_out as u128;
        let new_reserve_in = reserve_in + amount_after_fee;
        let new_reserve_out = (k / new_reserve_in as u128) as u64;
        let shares_out = reserve_out.saturating_sub(new_reserve_out);

        require!(shares_out >= min_shares_out, MarketError::SlippageExceeded);
        require!(shares_out >= MIN_SHARES_OUTPUT, MarketError::OutputTooSmall);

        // Transfer SOL to vault
        let cpi_context = CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            anchor_lang::system_program::Transfer {
                from: ctx.accounts.user.to_account_info(),
                to: ctx.accounts.vault.to_account_info(),
            },
        );
        anchor_lang::system_program::transfer(cpi_context, amount_in)?;

        // Update pool state
        // Add fee is effectively added to the pool by not being in reserves math
        // but we should track it for stats
        pool.total_fees_collected += fee;

        // Update reserves
        match side {
            Outcome::Yes => {
                pool.no_reserve = new_reserve_in;
                pool.yes_reserve = new_reserve_out;
            }
            Outcome::No => {
                pool.yes_reserve = new_reserve_in;
                pool.no_reserve = new_reserve_out;
            }
        }

        // Update market totals
        let market = &mut ctx.accounts.market;
        match side {
            Outcome::Yes => market.total_yes_shares += shares_out,
            Outcome::No => market.total_no_shares += shares_out,
        }

        // Update or create position
        let position = &mut ctx.accounts.position;
        if position.user == Pubkey::default() {
            position.user = ctx.accounts.user.key();
            position.market = market.key();
            position.bump = ctx.bumps.position;
        }

        // Update position shares
        let current_price = get_price_for_side(pool, side)?;
        match side {
            Outcome::Yes => {
                let old_shares = position.yes_shares;
                let new_shares = old_shares + shares_out;
                if new_shares > 0 {
                    position.yes_avg_price = ((position.yes_avg_price as u128 * old_shares as u128
                        + current_price as u128 * shares_out as u128)
                        / new_shares as u128) as u64;
                }
                position.yes_shares = new_shares;
            }
            Outcome::No => {
                let old_shares = position.no_shares;
                let new_shares = old_shares + shares_out;
                if new_shares > 0 {
                    position.no_avg_price = ((position.no_avg_price as u128 * old_shares as u128
                        + current_price as u128 * shares_out as u128)
                        / new_shares as u128) as u64;
                }
                position.no_shares = new_shares;
            }
        }

        msg!(
            "Bought {} {:?} shares for {} lamports",
            shares_out,
            side,
            amount_in
        );
        Ok(())
    }

    /// Sell YES or NO shares back to the AMM
    pub fn sell_shares(
        ctx: Context<Trade>,
        side: Outcome,
        shares_in: u64,
        min_amount_out: u64,
    ) -> Result<()> {
        require!(
            ctx.accounts.market.status == MarketStatus::Active,
            MarketError::MarketNotActive
        );
        require!(shares_in > 0, MarketError::InvalidAmount);

        // Verify user has enough shares
        let position = &ctx.accounts.position;
        match side {
            Outcome::Yes => require!(
                position.yes_shares >= shares_in,
                MarketError::InsufficientShares
            ),
            Outcome::No => require!(
                position.no_shares >= shares_in,
                MarketError::InsufficientShares
            ),
        }

        let pool = &mut ctx.accounts.pool;

        require!(
            pool.yes_reserve > 0 && pool.no_reserve > 0,
            MarketError::PoolNotInitialized
        );

        // Calculate output using constant product formula
        let (reserve_in, reserve_out) = match side {
            Outcome::Yes => (pool.yes_reserve, pool.no_reserve),
            Outcome::No => (pool.no_reserve, pool.yes_reserve),
        };

        let k = reserve_in as u128 * reserve_out as u128;
        let new_reserve_in = reserve_in + shares_in;
        let new_reserve_out = (k / new_reserve_in as u128) as u64;
        let amount_out_before_fee = reserve_out.saturating_sub(new_reserve_out);

        let fee = amount_out_before_fee * LP_FEE_BPS / BASIS_POINTS;
        let amount_out = amount_out_before_fee - fee;

        require!(amount_out >= min_amount_out, MarketError::SlippageExceeded);
        require!(amount_out >= MIN_SHARES_OUTPUT, MarketError::OutputTooSmall);

        // Check vault balance
        let vault_lamports = ctx.accounts.vault.lamports();
        require!(
            vault_lamports >= amount_out,
            MarketError::InsufficientVaultFunds
        );

        // Transfer SOL from vault to user
        let bump = ctx.bumps.vault;
        let bump_slice = &[bump];
        let market_key = ctx.accounts.market.key();
        let seeds = &[VAULT_SEED, market_key.as_ref(), bump_slice];
        let signer_seeds = &[&seeds[..]];

        let cpi_context = CpiContext::new_with_signer(
            ctx.accounts.system_program.to_account_info(),
            anchor_lang::system_program::Transfer {
                from: ctx.accounts.vault.to_account_info(),
                to: ctx.accounts.user.to_account_info(),
            },
            signer_seeds,
        );
        anchor_lang::system_program::transfer(cpi_context, amount_out)?;

        // Update pool state
        let pool = &mut ctx.accounts.pool;
        match side {
            Outcome::Yes => {
                pool.yes_reserve = new_reserve_in;
                pool.no_reserve = new_reserve_out;
            }
            Outcome::No => {
                pool.no_reserve = new_reserve_in;
                pool.yes_reserve = new_reserve_out;
            }
        }
        pool.total_fees_collected += fee;

        // Update market totals
        let market = &mut ctx.accounts.market;
        match side {
            Outcome::Yes => {
                market.total_yes_shares = market.total_yes_shares.saturating_sub(shares_in)
            }
            Outcome::No => {
                market.total_no_shares = market.total_no_shares.saturating_sub(shares_in)
            }
        }

        // Update position
        let position = &mut ctx.accounts.position;
        match side {
            Outcome::Yes => position.yes_shares = position.yes_shares.saturating_sub(shares_in),
            Outcome::No => position.no_shares = position.no_shares.saturating_sub(shares_in),
        }

        msg!(
            "Sold {} {:?} shares for {} lamports",
            shares_in,
            side,
            amount_out
        );
        Ok(())
    }

    /// Resolve the market using Pyth oracle price feed
    pub fn resolve_market(ctx: Context<ResolveMarket>) -> Result<()> {
        let market = &ctx.accounts.market;

        require!(
            market.status == MarketStatus::Active,
            MarketError::MarketNotActive
        );
        require!(
            Clock::get()?.unix_timestamp >= market.expiration + RESOLUTION_DELAY,
            MarketError::MarketNotExpired
        );

        // Read price from Pyth oracle
        let price_account_info = &ctx.accounts.pyth_price_account;
        let price_feed = SolanaPriceAccount::account_info_to_feed(price_account_info)
            .map_err(|_| MarketError::InvalidOraclePrice)?;

        let current_price = price_feed
            .get_price_no_older_than(
                Clock::get()?.unix_timestamp,
                300, // 5 minutes max staleness
            )
            .ok_or(MarketError::InvalidOraclePrice)?;

        // Check confidence interval
        require!(
            current_price.conf <= market.max_confidence,
            MarketError::ConfidenceTooHigh
        );

        // Determine outcome
        let outcome = if current_price.price >= market.strike_price {
            Outcome::Yes
        } else {
            Outcome::No
        };

        // Update market state
        let market = &mut ctx.accounts.market;
        market.status = MarketStatus::Resolved;
        market.outcome = Some(outcome);
        market.resolution_price = Some(current_price.price);
        market.resolution_timestamp = Some(Clock::get()?.unix_timestamp);

        msg!(
            "Market resolved: {:?} (price: {}, strike: {})",
            outcome,
            current_price.price,
            market.strike_price
        );
        Ok(())
    }

    /// Claim winnings after market resolution
    pub fn claim_winnings(ctx: Context<ClaimWinnings>) -> Result<()> {
        let market = &ctx.accounts.market;
        let position = &ctx.accounts.position;

        require!(
            market.status == MarketStatus::Resolved,
            MarketError::MarketNotResolved
        );
        require!(
            position.user == ctx.accounts.user.key(),
            MarketError::InvalidPosition
        );
        require!(!position.claimed, MarketError::AlreadyClaimed);

        let outcome = market.outcome.ok_or(MarketError::MarketNotResolved)?;

        // Calculate winnings based on outcome
        let winning_shares = match outcome {
            Outcome::Yes => position.yes_shares,
            Outcome::No => position.no_shares,
        };

        require!(winning_shares > 0, MarketError::NoWinnings);

        // Each winning share is worth 1 unit of collateral (1 lamport per share unit)
        let payout = winning_shares;

        // Check vault balance
        let vault_lamports = ctx.accounts.vault.lamports();
        require!(
            vault_lamports >= payout,
            MarketError::InsufficientVaultFunds
        );

        // Transfer winnings from vault
        let bump = ctx.bumps.vault;
        let bump_slice = &[bump];
        let market_key = ctx.accounts.market.key();
        let seeds = &[VAULT_SEED, market_key.as_ref(), bump_slice];
        let signer_seeds = &[&seeds[..]];

        let cpi_context = CpiContext::new_with_signer(
            ctx.accounts.system_program.to_account_info(),
            anchor_lang::system_program::Transfer {
                from: ctx.accounts.vault.to_account_info(),
                to: ctx.accounts.user.to_account_info(),
            },
            signer_seeds,
        );
        anchor_lang::system_program::transfer(cpi_context, payout)?;

        // Mark position as claimed
        let position = &mut ctx.accounts.position;
        position.claimed = true;

        msg!(
            "Claimed {} lamports for {} winning shares",
            payout,
            winning_shares
        );
        Ok(())
    }

    /// Cancel a market that has not been resolved
    pub fn cancel_market(ctx: Context<CancelMarket>) -> Result<()> {
        let market = &mut ctx.accounts.market;
        require!(
            market.status == MarketStatus::Active,
            MarketError::MarketNotActive
        );
        require!(
            market.authority == ctx.accounts.authority.key(),
            MarketError::Unauthorized
        );

        market.status = MarketStatus::Cancelled;
        msg!("Market cancelled by authority");
        Ok(())
    }

    // ========================================
    // Ephemeral Rollup Functions
    // ========================================

    /// Delegate market and pool to ephemeral rollup for high-speed trading
    pub fn delegate_market(ctx: Context<DelegateMarket>) -> Result<()> {
        require!(
            ctx.accounts.market.status == MarketStatus::Active,
            MarketError::MarketNotActive
        );

        ctx.accounts.delegate_pda(
            &ctx.accounts.payer,
            &[MARKET_SEED, ctx.accounts.market.market_id.as_ref()],
            DelegateConfig {
                validator: ctx.remaining_accounts.first().map(|acc| acc.key()),
                ..Default::default()
            },
        )?;

        msg!("Market delegated to ephemeral rollup");
        Ok(())
    }

    /// Commit current state from ephemeral rollup to L1
    pub fn commit_state(ctx: Context<CommitState>) -> Result<()> {
        commit_accounts(
            &ctx.accounts.payer,
            vec![
                &ctx.accounts.market.to_account_info(),
                &ctx.accounts.pool.to_account_info(),
            ],
            &ctx.accounts.magic_context,
            &ctx.accounts.magic_program,
        )?;

        msg!("State committed to L1");
        Ok(())
    }

    /// Undelegate market from ephemeral rollup (commit and return to L1)
    pub fn undelegate_market(ctx: Context<CommitState>) -> Result<()> {
        commit_and_undelegate_accounts(
            &ctx.accounts.payer,
            vec![
                &ctx.accounts.market.to_account_info(),
                &ctx.accounts.pool.to_account_info(),
            ],
            &ctx.accounts.magic_context,
            &ctx.accounts.magic_program,
        )?;

        msg!("Market undelegated from ephemeral rollup");
        Ok(())
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

fn get_price_for_side(pool: &Pool, side: Outcome) -> Result<u64> {
    let total = pool.yes_reserve + pool.no_reserve;
    if total == 0 {
        return Ok(PRICE_DECIMALS / 2); // 0.5 default
    }
    match side {
        Outcome::Yes => {
            Ok((pool.no_reserve as u128 * PRICE_DECIMALS as u128 / total as u128) as u64)
        }
        Outcome::No => {
            Ok((pool.yes_reserve as u128 * PRICE_DECIMALS as u128 / total as u128) as u64)
        }
    }
}

// ============================================================================
// Account Structs
// ============================================================================

#[derive(Accounts)]
#[instruction(market_id: [u8; 32])]
pub struct CreateMarket<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + Market::INIT_SPACE,
        seeds = [MARKET_SEED, market_id.as_ref()],
        bump
    )]
    pub market: Account<'info, Market>,

    /// CHECK: Pyth price account - validated by Pyth SDK when reading
    pub pyth_price_account: AccountInfo<'info>,

    #[account(mut)]
    pub authority: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct InitializePool<'info> {
    #[account(mut)]
    pub market: Account<'info, Market>,

    #[account(
        init,
        payer = authority,
        space = 8 + Pool::INIT_SPACE,
        seeds = [POOL_SEED, market.key().as_ref()],
        bump
    )]
    pub pool: Account<'info, Pool>,

    /// CHECK: Vault PDA for holding SOL
    #[account(
        init,
        payer = authority,
        space = 0,
        seeds = [VAULT_SEED, market.key().as_ref()],
        bump
    )]
    pub vault: AccountInfo<'info>,

    #[account(
        init,
        payer = authority,
        space = 8 + LPPosition::INIT_SPACE,
        seeds = [LP_POSITION_SEED, pool.key().as_ref(), authority.key().as_ref()],
        bump
    )]
    pub lp_position: Account<'info, LPPosition>,

    #[account(mut)]
    pub authority: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ModifyLiquidity<'info> {
    #[account(mut)]
    pub market: Account<'info, Market>,

    #[account(mut, seeds = [POOL_SEED, market.key().as_ref()], bump = pool.bump)]
    pub pool: Account<'info, Pool>,

    /// CHECK: Vault PDA
    #[account(mut, seeds = [VAULT_SEED, market.key().as_ref()], bump)]
    pub vault: AccountInfo<'info>,

    #[account(
        init_if_needed,
        payer = user,
        space = 8 + LPPosition::INIT_SPACE,
        seeds = [LP_POSITION_SEED, pool.key().as_ref(), user.key().as_ref()],
        bump
    )]
    pub lp_position: Account<'info, LPPosition>,

    #[account(mut)]
    pub user: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Trade<'info> {
    #[account(mut)]
    pub market: Account<'info, Market>,

    #[account(mut, seeds = [POOL_SEED, market.key().as_ref()], bump = pool.bump)]
    pub pool: Account<'info, Pool>,

    /// CHECK: Vault PDA
    #[account(mut, seeds = [VAULT_SEED, market.key().as_ref()], bump)]
    pub vault: AccountInfo<'info>,

    #[account(
        init_if_needed,
        payer = user,
        space = 8 + Position::INIT_SPACE,
        seeds = [POSITION_SEED, market.key().as_ref(), user.key().as_ref()],
        bump
    )]
    pub position: Account<'info, Position>,

    #[account(mut)]
    pub user: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ResolveMarket<'info> {
    #[account(mut)]
    pub market: Account<'info, Market>,

    /// CHECK: Pyth price account - validated when reading
    #[account(constraint = pyth_price_account.key() == market.pyth_price_account)]
    pub pyth_price_account: AccountInfo<'info>,

    #[account(mut)]
    pub resolver: Signer<'info>,
}

#[derive(Accounts)]
pub struct ClaimWinnings<'info> {
    #[account(mut)]
    pub market: Account<'info, Market>,

    /// CHECK: Vault PDA
    #[account(mut, seeds = [VAULT_SEED, market.key().as_ref()], bump)]
    pub vault: AccountInfo<'info>,

    #[account(mut, seeds = [POSITION_SEED, market.key().as_ref(), user.key().as_ref()], bump = position.bump)]
    pub position: Account<'info, Position>,

    #[account(mut)]
    pub user: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct CancelMarket<'info> {
    #[account(mut)]
    pub market: Account<'info, Market>,

    #[account(mut)]
    pub authority: Signer<'info>,
}

#[delegate]
#[derive(Accounts)]
pub struct DelegateMarket<'info> {
    pub payer: Signer<'info>,

    /// CHECK: Market PDA to delegate
    #[account(mut, del, seeds = [MARKET_SEED, market.market_id.as_ref()], bump = market.bump)]
    pub pda: AccountInfo<'info>,

    #[account(mut)]
    pub market: Account<'info, Market>,
}

#[commit]
#[derive(Accounts)]
pub struct CommitState<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(mut)]
    pub market: Account<'info, Market>,

    #[account(mut, seeds = [POOL_SEED, market.key().as_ref()], bump = pool.bump)]
    pub pool: Account<'info, Pool>,
}

// ============================================================================
// State Accounts
// ============================================================================

#[account]
#[derive(InitSpace)]
pub struct Market {
    /// Market creator/authority
    pub authority: Pubkey,
    /// Unique market identifier
    pub market_id: [u8; 32],
    /// Strike price for resolution (scaled by 10^8 like Pyth)
    pub strike_price: i64,
    /// Unix timestamp when market expires
    pub expiration: i64,
    /// Pyth price account to use for resolution
    pub pyth_price_account: Pubkey,
    /// Maximum confidence interval for resolution
    pub max_confidence: u64,
    /// Current market status
    pub status: MarketStatus,
    /// Resolved outcome (if resolved)
    pub outcome: Option<Outcome>,
    /// Price at resolution (from Pyth)
    pub resolution_price: Option<i64>,
    /// Timestamp of resolution
    pub resolution_timestamp: Option<i64>,
    /// Total YES shares outstanding
    pub total_yes_shares: u64,
    /// Total NO shares outstanding
    pub total_no_shares: u64,
    /// Market description
    #[max_len(128)]
    pub description: String,
    /// Bump seed
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct Pool {
    /// Associated market
    pub market: Pubkey,
    /// YES side reserve (virtual)
    pub yes_reserve: u64,
    /// NO side reserve (virtual)
    pub no_reserve: u64,
    /// Total liquidity deposited
    pub total_liquidity: u64,
    /// Cumulative fees collected
    pub total_fees_collected: u64,
    /// Total LP tokens minted
    pub lp_token_supply: u64,
    /// Bump seed
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct LPPosition {
    /// Position owner
    pub user: Pubkey,
    /// Associated pool
    pub pool: Pubkey,
    /// LP tokens owned
    pub lp_tokens: u64,
    /// Bump seed
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct Position {
    /// Position owner
    pub user: Pubkey,
    /// Associated market
    pub market: Pubkey,
    /// YES shares held
    pub yes_shares: u64,
    /// NO shares held
    pub no_shares: u64,
    /// Average entry price for YES (scaled by PRICE_DECIMALS)
    pub yes_avg_price: u64,
    /// Average entry price for NO (scaled by PRICE_DECIMALS)
    pub no_avg_price: u64,
    /// Whether winnings have been claimed
    pub claimed: bool,
    /// Bump seed
    pub bump: u8,
}

// ============================================================================
// Enums
// ============================================================================

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, InitSpace, Debug)]
pub enum MarketStatus {
    Active,
    Resolved,
    Cancelled,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, InitSpace, Debug)]
pub enum Outcome {
    Yes,
    No,
}

// ============================================================================
// Errors
// ============================================================================

#[error_code]
pub enum MarketError {
    #[msg("Market has already expired")]
    InvalidExpiration,
    #[msg("Description exceeds maximum length")]
    DescriptionTooLong,
    #[msg("Insufficient initial liquidity")]
    InsufficientLiquidity,
    #[msg("Market is not active")]
    MarketNotActive,
    #[msg("Invalid amount")]
    InvalidAmount,
    #[msg("Slippage tolerance exceeded")]
    SlippageExceeded,
    #[msg("Market has not expired yet")]
    MarketNotExpired,
    #[msg("Oracle confidence interval too high")]
    ConfidenceTooHigh,
    #[msg("Market has not been resolved")]
    MarketNotResolved,
    #[msg("Invalid position")]
    InvalidPosition,
    #[msg("No winnings to claim")]
    NoWinnings,
    #[msg("Invalid oracle price")]
    InvalidOraclePrice,
    #[msg("Insufficient shares to sell")]
    InsufficientShares,
    #[msg("Already claimed winnings")]
    AlreadyClaimed,
    #[msg("Insufficient vault funds")]
    InsufficientVaultFunds,
    #[msg("Pool not initialized")]
    PoolNotInitialized,
    #[msg("Trade exceeds maximum size")]
    TradeExceedsMaxSize,
    #[msg("Unauthorized access")]
    Unauthorized,
    #[msg("Market cannot be cancelled")]
    MarketCannotBeCancelled,
    #[msg("Output amount too small")]
    OutputTooSmall,
}
