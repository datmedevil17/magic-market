# Prediction Market Platform - Feature Documentation

A high-performance Solana prediction market combining **Pyth Network oracles** for real-time price feeds and **Magic Block ephemeral rollups** for instant trade execution.

---

## Core Features

### 🎯 Binary Outcome Markets
Create markets with YES/NO outcomes that resolve based on real-world price data from Pyth oracles.

- **Oracle-backed resolution**: Markets resolve automatically when Pyth prices cross predefined thresholds
- **Confidence verification**: Only resolves when price confidence meets quality requirements
- **Customizable parameters**: Strike price, expiration, max confidence interval

### ⚡ Instant Trade Execution (Ephemeral Rollups)
Trades execute in **sub-10ms** on Magic Block's ephemeral rollups with zero gas fees.

- **Delegate → Trade → Commit → Undelegate** workflow
- State batched and committed to Solana L1 with cryptographic proofs
- Eliminates front-running through off-chain execution

### 💧 Automated Market Maker (AMM)
Constant product AMM (`x * y = k`) with dynamic pricing.

- **LP tokens** for passive market making
- **0.3% trading fee** distributed to liquidity providers
- Prices auto-balance based on trading activity

### 📊 Position Management
Complete tracking of user holdings and P/L.

- Entry price averaging
- Real-time profit/loss against live oracle data
- Automatic claim system for winning positions

---

## User Flow Stories

### Story 1: The Market Creator 🏗️

**Sarah is a DeFi protocol founder** who wants to create a prediction market for her token's price milestone.

**Setup Phase:**
Sarah connects her Phantom wallet and navigates to "Create Market". She specifies: "Will TOKEN be above $50 on March 1st?" with a strike price of $50, selects the Pyth TOKEN/USD price feed, and sets expiration to March 1st, 2025 at 00:00 UTC. She sets the maximum confidence interval to $0.50, meaning the market will only resolve if Pyth's price uncertainty is within 50 cents. After reviewing the parameters, she signs the transaction and pays her market creation fee of 0.05 SOL.

**Liquidity Provision:**
Her market is created but has no liquidity. Sarah decides to bootstrap it by depositing 10 SOL of initial liquidity, receiving LP tokens in return. This creates equal YES and NO reserves, pricing both outcomes at 50% initially. Other LPs see the opportunity and add more liquidity, earning their share of trading fees. Sarah's LP tokens will be redeemable for her proportional share of the pool plus accumulated fees after trading activity.

**Market Lifecycle:**
Over the following weeks, traders buy and sell YES/NO shares, generating fees for LPs. When March 1st arrives, anyone can call `resolve_market`, which reads the Pyth oracle price. If TOKEN is above $50, YES holders win; otherwise, NO holders win. Sarah can withdraw her LP tokens anytime before resolution, or wait until after to claim her fees.

---

### Story 2: The Day Trader ⚡

**Marcus is an active crypto trader** who spotted an arbitrage opportunity between prediction market odds and his technical analysis.

**Discovery:**
Marcus sees a prediction market "Will BTC be above $100K on Friday?" trading at 35% YES probability. His analysis suggests 60%+ probability based on current momentum and upcoming ETF news. He calculates the expected value: buying YES at $0.35 with a 60% chance of winning $1.00 gives him +$0.25 EV per share. He sets his maximum slippage tolerance to 2% and prepares to buy.

**High-Speed Trading:**
Marcus clicks "Enable Fast Trading" which delegates the market to Magic Block's ephemeral rollup. His trades now execute in under 10 milliseconds with zero gas fees—100x faster than typical Solana transactions. He buys 1000 YES shares for $350, watching the price impact on his order. The AMM shifts the YES price from $0.35 to $0.38. He places a limit order to sell half at $0.50 for guaranteed profit. Throughout the day, he makes 15 trades as news breaks, each confirming instantly.

**Settlement:**
At end of day, Marcus clicks "Settle to L1" which commits all his ephemeral rollup trades to Solana mainnet in a single transaction. His position shows: 500 YES shares at $0.38 average, current price $0.52, unrealized profit of $70 (37% gain). He can hold until resolution or sell anytime. If BTC is above $100K on Friday, his 500 shares become worth $500, turning his $190 investment into $310 profit.

---

### Story 3: The Passive LP 💰

**Elena wants passive yield** without actively trading. She discovered prediction market liquidity provision offers higher APY than traditional DeFi pools.

**Research:**
Elena browses active markets and sees "Will SOL hit $200 by April?" with $500K total value locked, 0.3% fee per trade, and ~40% implied volatility. The market has high trading volume—perfect for fee generation. She calculates that at current volume ($50K daily), LPs earn approximately $150/day in fees. With $500K TVL, that's 11% APY just from fees, plus any gains from price movements.

**LP Position:**
Elena deposits 5 SOL ($1000) into the liquidity pool. She receives 1,414 LP tokens representing her 0.2% share of the pool. Her deposit is split 50/50 into YES and NO reserves—she's now market-neutral, profiting from trading activity regardless of outcome. Every trade in this market generates fees that accumulate in her LP position. She watches the "Fees Earned" counter tick up with each trade.

**Outcomes:**
After one month: $1000 initial deposit + $25 fees earned - $5 impermanent loss = $1020 value (2% monthly, 24% APY). When the market resolves, Elena's position automatically converts to the winning outcome proportionally. She withdraws her LP tokens, receiving 5.1 SOL back. Even though she was neutral on the prediction, she profited from providing liquidity. She reinvests into the next high-volume market, compounding her yields.

---

## Technical Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                    User Interface (Frontend)                     │
└─────────────────────────────────────────────────────────────────┘
                              │
         ┌────────────────────┼────────────────────┐
         ▼                    ▼                    ▼
┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐
│  Create Market  │  │  Trade (Fast)   │  │  Resolve/Claim  │
│  Add Liquidity  │  │  via Ephemeral  │  │  via Pyth       │
│  (Solana L1)    │  │  Rollup         │  │  Oracle         │
└─────────────────┘  └─────────────────┘  └─────────────────┘
         │                    │                    │
         ▼                    ▼                    ▼
┌─────────────────────────────────────────────────────────────────┐
│              Prediction Market Smart Contract                    │
│  ┌───────────┐  ┌───────────┐  ┌───────────┐  ┌───────────┐    │
│  │  Market   │  │   Pool    │  │ Position  │  │   Vault   │    │
│  │  Account  │  │  Account  │  │  Account  │  │  (SOL)    │    │
│  └───────────┘  └───────────┘  └───────────┘  └───────────┘    │
└─────────────────────────────────────────────────────────────────┘
         │                    │                    │
         ▼                    ▼                    ▼
┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐
│  Magic Block    │  │  Solana L1      │  │  Pyth Network   │
│  Ephemeral      │  │  Base Layer     │  │  Oracles        │
│  Rollup         │  │  Settlement     │  │  Price Feeds    │
└─────────────────┘  └─────────────────┘  └─────────────────┘
```

---

## Instructions Reference

| Instruction | Description | When to Use |
|-------------|-------------|-------------|
| `create_market` | Create new binary market | Market creator setup |
| `initialize_pool` | Bootstrap AMM liquidity | After market creation |
| `add_liquidity` | Deposit as LP | Passive yield seekers |
| `remove_liquidity` | Withdraw LP position | Exit LP position |
| `buy_shares` | Purchase YES/NO shares | Active trading |
| `sell_shares` | Sell shares back to AMM | Take profit/loss |
| `delegate_market` | Move to ephemeral rollup | Enable fast trading |
| `commit_state` | Persist ER state to L1 | Checkpoint state |
| `undelegate_market` | Return to L1 | Before resolution |
| `resolve_market` | Settle via Pyth oracle | At expiration |
| `claim_winnings` | Collect payout | After resolution |

---

## Security Considerations

- **Oracle Integrity**: Multi-publisher consensus from Pyth prevents manipulation
- **Confidence Checks**: Markets only resolve with high-quality price data
- **Front-run Protection**: Off-chain execution prevents MEV attacks
- **Trustless Settlement**: Cryptographic proofs verify ER state on L1
- **LP Protection**: Impermanent loss mitigated by trading fees
