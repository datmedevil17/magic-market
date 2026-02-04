import * as anchor from "@coral-xyz/anchor";
import { Program, web3, BN } from "@coral-xyz/anchor";
import { LAMPORTS_PER_SOL, PublicKey, SystemProgram, Keypair } from "@solana/web3.js";
import { expect } from "chai";
import { PredictionMarket } from "../target/types/prediction_market";

describe("prediction_market", () => {
  console.log("prediction_market.ts - Test Suite");

  // Configure providers
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  // Ephemeral Rollup provider
  const providerEphemeralRollup = new anchor.AnchorProvider(
    new anchor.web3.Connection(
      process.env.EPHEMERAL_PROVIDER_ENDPOINT || "https://rpc.magicblock.app/devnet",
      {
        wsEndpoint: process.env.EPHEMERAL_WS_ENDPOINT || "wss://devnet.magicblock.app/",
      }
    ),
    anchor.Wallet.local()
  );

  console.log("Base Layer Connection:", provider.connection.rpcEndpoint);
  console.log("Ephemeral Rollup Connection:", providerEphemeralRollup.connection.rpcEndpoint);
  console.log("Wallet:", anchor.Wallet.local().publicKey.toString());

  const program = anchor.workspace.PredictionMarket as Program<PredictionMarket>;
  const authority = provider.wallet;

  // Test market parameters
  const marketId = new Uint8Array(32);
  marketId[0] = 1; // Simple unique ID

  // Mock Pyth price account (for testing - use real Pyth feed in production)
  const mockPythPriceAccount = Keypair.generate();

  const strikePrice = new BN(100_00000000); // $100 with 8 decimals (Pyth format)
  const maxConfidence = new BN(1_00000000); // $1 max confidence
  const description = "Will SOL be above $100 on expiration?";

  // Calculate expiration (1 hour from now for testing)
  const expiration = new BN(Math.floor(Date.now() / 1000) + 3600);

  // Derive PDAs
  let marketPDA: PublicKey;
  let poolPDA: PublicKey;
  let vaultPDA: PublicKey;
  let positionPDA: PublicKey;

  before(async function () {
    // Log balance
    const balance = await provider.connection.getBalance(anchor.Wallet.local().publicKey);
    console.log("Current balance:", balance / LAMPORTS_PER_SOL, "SOL\n");

    // Derive all PDAs
    [marketPDA] = PublicKey.findProgramAddressSync(
      [Buffer.from("market"), Buffer.from(marketId)],
      program.programId
    );
    console.log("Market PDA:", marketPDA.toString());

    [poolPDA] = PublicKey.findProgramAddressSync(
      [Buffer.from("pool"), marketPDA.toBuffer()],
      program.programId
    );
    console.log("Pool PDA:", poolPDA.toString());

    [vaultPDA] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), marketPDA.toBuffer()],
      program.programId
    );
    console.log("Vault PDA:", vaultPDA.toString());

    [positionPDA] = PublicKey.findProgramAddressSync(
      [Buffer.from("position"), marketPDA.toBuffer(), authority.publicKey.toBuffer()],
      program.programId
    );
    console.log("Position PDA:", positionPDA.toString());
  });

  // ========================================
  // Market Lifecycle Tests
  // ========================================

  describe("Market Creation", () => {
    it("creates a new prediction market", async () => {
      const start = Date.now();
      const tx = await program.methods
        .createMarket(
          Array.from(marketId),
          strikePrice,
          expiration,
          maxConfidence,
          description
        )
        .accounts({
          market: marketPDA,
          pythPriceAccount: mockPythPriceAccount.publicKey,
          authority: authority.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .rpc({ skipPreflight: true });

      const duration = Date.now() - start;
      console.log(`${duration}ms - Create Market tx: ${tx}`);

      // Verify market state
      const market = await program.account.market.fetch(marketPDA);
      expect(market.authority.toBase58()).to.equal(authority.publicKey.toBase58());
      expect(market.strikePrice.toNumber()).to.equal(strikePrice.toNumber());
      expect(market.description).to.equal(description);
      expect(market.status).to.deep.equal({ active: {} });
      expect(market.pythPriceAccount.toBase58()).to.equal(mockPythPriceAccount.publicKey.toBase58());
    });
  });

  describe("Liquidity Pool", () => {
    it("initializes the liquidity pool", async () => {
      const initialLiquidity = new BN(1_000_000_000); // 1 SOL

      const start = Date.now();
      const tx = await program.methods
        .initializePool(initialLiquidity)
        .accounts({
          market: marketPDA,
          pool: poolPDA,
          vault: vaultPDA,
          authority: authority.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .rpc({ skipPreflight: true });

      const duration = Date.now() - start;
      console.log(`${duration}ms - Initialize Pool tx: ${tx}`);

      // Verify pool state
      const pool = await program.account.pool.fetch(poolPDA);
      expect(pool.market.toBase58()).to.equal(marketPDA.toBase58());
      expect(pool.yesReserve.toNumber()).to.equal(initialLiquidity.toNumber());
      expect(pool.noReserve.toNumber()).to.equal(initialLiquidity.toNumber());
      expect(pool.totalLiquidity.toNumber()).to.equal(initialLiquidity.toNumber() * 2);
    });

    it("allows adding more liquidity", async () => {
      const addAmount = new BN(500_000_000); // 0.5 SOL

      const poolBefore = await program.account.pool.fetch(poolPDA);

      const tx = await program.methods
        .addLiquidity(addAmount)
        .accounts({
          market: marketPDA,
          pool: poolPDA,
          vault: vaultPDA,
          user: authority.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .rpc({ skipPreflight: true });

      console.log("Add Liquidity tx:", tx);

      const poolAfter = await program.account.pool.fetch(poolPDA);
      expect(poolAfter.totalLiquidity.toNumber()).to.be.greaterThan(
        poolBefore.totalLiquidity.toNumber()
      );
    });
  });

  describe("Trading (AMM)", () => {
    it("buys YES shares", async () => {
      const amountIn = new BN(100_000_000); // 0.1 SOL
      const minSharesOut = new BN(1); // Minimum 1 share

      const start = Date.now();
      const tx = await program.methods
        .buyShares({ yes: {} }, amountIn, minSharesOut)
        .accounts({
          market: marketPDA,
          pool: poolPDA,
          vault: vaultPDA,
          position: positionPDA,
          user: authority.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .rpc({ skipPreflight: true });

      const duration = Date.now() - start;
      console.log(`${duration}ms - Buy YES shares tx: ${tx}`);

      // Verify position
      const position = await program.account.position.fetch(positionPDA);
      expect(position.yesShares.toNumber()).to.be.greaterThan(0);
      console.log("YES shares bought:", position.yesShares.toNumber());
    });

    it("buys NO shares", async () => {
      const amountIn = new BN(100_000_000); // 0.1 SOL
      const minSharesOut = new BN(1);

      const tx = await program.methods
        .buyShares({ no: {} }, amountIn, minSharesOut)
        .accounts({
          market: marketPDA,
          pool: poolPDA,
          vault: vaultPDA,
          position: positionPDA,
          user: authority.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .rpc({ skipPreflight: true });

      console.log("Buy NO shares tx:", tx);

      const position = await program.account.position.fetch(positionPDA);
      expect(position.noShares.toNumber()).to.be.greaterThan(0);
      console.log("NO shares bought:", position.noShares.toNumber());
    });

    it("sells YES shares", async () => {
      const position = await program.account.position.fetch(positionPDA);
      const sharesToSell = new BN(Math.floor(position.yesShares.toNumber() / 2)); // Sell half
      const minAmountOut = new BN(1);

      const start = Date.now();
      const tx = await program.methods
        .sellShares({ yes: {} }, sharesToSell, minAmountOut)
        .accounts({
          market: marketPDA,
          pool: poolPDA,
          vault: vaultPDA,
          position: positionPDA,
          user: authority.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .rpc({ skipPreflight: true });

      const duration = Date.now() - start;
      console.log(`${duration}ms - Sell YES shares tx: ${tx}`);

      const positionAfter = await program.account.position.fetch(positionPDA);
      expect(positionAfter.yesShares.toNumber()).to.be.lessThan(position.yesShares.toNumber());
    });
  });

  // ========================================
  // Ephemeral Rollup Tests
  // ========================================

  describe("Ephemeral Rollup Integration", () => {
    it("delegates market to ephemeral rollup", async () => {
      // Skip on localnet if ER not available
      if (provider.connection.rpcEndpoint.includes("localhost") || 
          provider.connection.rpcEndpoint.includes("127.0.0.1")) {
        console.log("Skipping ER tests on localnet");
        return;
      }

      const remainingAccounts = [
        {
          pubkey: new web3.PublicKey("mAGicPQYBMvcYveUZA5F5UNNwyHvfYh5xkLS2Fr1mev"),
          isSigner: false,
          isWritable: false,
        },
      ];

      const start = Date.now();
      const tx = await program.methods
        .delegateMarket()
        .accounts({
          payer: authority.publicKey,
          pda: marketPDA,
          market: marketPDA,
        })
        .remainingAccounts(remainingAccounts)
        .rpc({ skipPreflight: true });

      const duration = Date.now() - start;
      console.log(`${duration}ms - Delegate Market tx: ${tx}`);
    });

    it("executes high-speed trades on ephemeral rollup", async () => {
      if (provider.connection.rpcEndpoint.includes("localhost") || 
          provider.connection.rpcEndpoint.includes("127.0.0.1")) {
        console.log("Skipping ER trades on localnet");
        return;
      }

      const start = Date.now();
      
      // Build transaction
      let tx = await program.methods
        .buyShares({ yes: {} }, new BN(50_000_000), new BN(1))
        .accounts({
          market: marketPDA,
          pool: poolPDA,
          vault: vaultPDA,
          position: positionPDA,
          user: authority.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .transaction();

      // Set up for ER connection
      tx.feePayer = providerEphemeralRollup.wallet.publicKey;
      tx.recentBlockhash = (await providerEphemeralRollup.connection.getLatestBlockhash()).blockhash;
      tx = await providerEphemeralRollup.wallet.signTransaction(tx);

      const txHash = await providerEphemeralRollup.connection.sendRawTransaction(
        tx.serialize(),
        { skipPreflight: true }
      );
      await providerEphemeralRollup.connection.confirmTransaction(txHash, "confirmed");

      const duration = Date.now() - start;
      console.log(`${duration}ms (ER) Buy shares tx: ${txHash}`);
    });

    it("commits state to L1", async () => {
      if (provider.connection.rpcEndpoint.includes("localhost") || 
          provider.connection.rpcEndpoint.includes("127.0.0.1")) {
        console.log("Skipping ER commit on localnet");
        return;
      }

      const start = Date.now();
      
      let tx = await program.methods
        .commitState()
        .accounts({
          payer: providerEphemeralRollup.wallet.publicKey,
          market: marketPDA,
          pool: poolPDA,
        })
        .transaction();

      tx.feePayer = providerEphemeralRollup.wallet.publicKey;
      tx.recentBlockhash = (await providerEphemeralRollup.connection.getLatestBlockhash()).blockhash;
      tx = await providerEphemeralRollup.wallet.signTransaction(tx);

      const txHash = await providerEphemeralRollup.connection.sendRawTransaction(
        tx.serialize(),
        { skipPreflight: true }
      );
      await providerEphemeralRollup.connection.confirmTransaction(txHash, "confirmed");

      const duration = Date.now() - start;
      console.log(`${duration}ms (ER) Commit state tx: ${txHash}`);
    });

    it("undelegates market from ephemeral rollup", async () => {
      if (provider.connection.rpcEndpoint.includes("localhost") || 
          provider.connection.rpcEndpoint.includes("127.0.0.1")) {
        console.log("Skipping ER undelegate on localnet");
        return;
      }

      const start = Date.now();
      
      let tx = await program.methods
        .undelegateMarket()
        .accounts({
          payer: providerEphemeralRollup.wallet.publicKey,
          market: marketPDA,
          pool: poolPDA,
        })
        .transaction();

      tx.feePayer = providerEphemeralRollup.wallet.publicKey;
      tx.recentBlockhash = (await providerEphemeralRollup.connection.getLatestBlockhash()).blockhash;
      tx = await providerEphemeralRollup.wallet.signTransaction(tx);

      const txHash = await providerEphemeralRollup.connection.sendRawTransaction(
        tx.serialize(),
        { skipPreflight: true }
      );
      await providerEphemeralRollup.connection.confirmTransaction(txHash, "confirmed");

      const duration = Date.now() - start;
      console.log(`${duration}ms (ER) Undelegate tx: ${txHash}`);
    });
  });

  // ========================================
  // Market Resolution Tests
  // ========================================

  describe("Market Resolution", () => {
    it("should reject resolution before expiration", async () => {
      try {
        await program.methods
          .resolveMarket()
          .accounts({
            market: marketPDA,
            pythPriceAccount: mockPythPriceAccount.publicKey,
            resolver: authority.publicKey,
          })
          .rpc();
        
        expect.fail("Should have thrown MarketNotExpired error");
      } catch (err: any) {
        // Expected to fail
        console.log("Resolution correctly rejected (market not expired)");
        expect(err.toString()).to.include("MarketNotExpired");
      }
    });
  });

  // ========================================
  // View Functions
  // ========================================

  describe("Market State Queries", () => {
    it("queries market state", async () => {
      const market = await program.account.market.fetch(marketPDA);
      
      console.log("\n--- Market State ---");
      console.log("Authority:", market.authority.toBase58());
      console.log("Strike Price:", market.strikePrice.toString(), "(10^8 scaled)");
      console.log("Expiration:", new Date(market.expiration.toNumber() * 1000).toISOString());
      console.log("Status:", JSON.stringify(market.status));
      console.log("Total YES Shares:", market.totalYesShares.toString());
      console.log("Total NO Shares:", market.totalNoShares.toString());
      console.log("Description:", market.description);
    });

    it("queries pool state with implied probabilities", async () => {
      const pool = await program.account.pool.fetch(poolPDA);
      
      console.log("\n--- Pool State ---");
      console.log("YES Reserve:", pool.yesReserve.toString());
      console.log("NO Reserve:", pool.noReserve.toString());
      console.log("Total Liquidity:", pool.totalLiquidity.toString());
      console.log("Total Fees:", pool.totalFeesCollected.toString());

      // Calculate implied probabilities
      const totalReserve = pool.yesReserve.toNumber() + pool.noReserve.toNumber();
      const yesProb = (pool.noReserve.toNumber() / totalReserve * 100).toFixed(2);
      const noProb = (pool.yesReserve.toNumber() / totalReserve * 100).toFixed(2);
      
      console.log(`Implied YES Probability: ${yesProb}%`);
      console.log(`Implied NO Probability: ${noProb}%`);
    });

    it("queries position state with P/L", async () => {
      const position = await program.account.position.fetch(positionPDA);
      const pool = await program.account.pool.fetch(poolPDA);
      
      console.log("\n--- Position State ---");
      console.log("User:", position.user.toBase58());
      console.log("YES Shares:", position.yesShares.toString());
      console.log("NO Shares:", position.noShares.toString());
      console.log("YES Avg Price:", position.yesAvgPrice.toString());
      console.log("NO Avg Price:", position.noAvgPrice.toString());
      console.log("Claimed:", position.claimed);

      // Calculate current prices
      const totalReserve = pool.yesReserve.toNumber() + pool.noReserve.toNumber();
      const currentYesPrice = pool.noReserve.toNumber() / totalReserve * 1_000_000;
      const currentNoPrice = pool.yesReserve.toNumber() / totalReserve * 1_000_000;

      console.log(`Current YES Price: ${(currentYesPrice / 1_000_000).toFixed(4)}`);
      console.log(`Current NO Price: ${(currentNoPrice / 1_000_000).toFixed(4)}`);
    });
  });
});
