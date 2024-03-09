const {
    LOCALHOST,
    tmpLedgerDir,
    localDeployPath
  } = require( '@metaplex-foundation/amman' );
  
  module.exports = {
    validator: {
      // By default Amman will pull the account data from the accountsCluster (can be overridden on a per account basis)
      accountsCluster: 'https://jarrett-devnet-8fa6.devnet.rpcpool.com/283aba57-34a4-4500-ba4d-1832ff9ca64a',
      accounts: [
          {
            label: 'Token Metadata Program',
            accountId:'metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s',
            // marking executable as true will cause Amman to pull the executable data account as well automatically
            executable: true,
          },
          {
            label: 'stacc',
            accountId:'7ihN8QaTfNoDTRTQGULCzbUT3PHwPDTu5Brcu4iT2paP',
            // By default executable is false and is not required to be in the config
            // executable: false,
          },
          {
            label: "bonk",
            accountId: "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263"
          },
          {
            label: "usdc",
            accountId: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
          },
          {
            label: "stacc's usdc",
            accountId: "CyytQ6ipQMabBpCJnZjK1PasxHN5Fg7PDhMvNCHfijto"
          },
          {
            label: "stacc's bonk",
            accountId: "4Ucsagtk8eSpYnMfa8sKug3QzrC6g76F4roiF9i5HECq"
          }
        ],
      killRunningValidators: true,
      programs: [
        { 
          label: 'Raydium Amm V3',
          programId: "6weQP6SNcqqk8KnQGcM2rzU1Xk9o9atJD8kvASVCrN55",
          deployPath: "/Users/jd/clammy/target/deploy/raydium_amm_v3.so"
        },
      ],
      jsonRpcUrl: LOCALHOST,
      websocketUrl: '',
      commitment: 'confirmed',
      ledgerDir: tmpLedgerDir(),
      resetLedger: true,
      verifyFees: false,
      detached: process.env.CI != null,
    }
    }
  