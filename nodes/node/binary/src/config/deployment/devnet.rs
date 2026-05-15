pub const NAME: &str = "devnet";

pub const SERIALIZED_DEPLOYMENT: &str = "
blend:
  common:
    num_blend_layers: 1
    minimum_network_size: 2
    protocol_name: /logos-blockchain-devnet-0.1.3-rc.6/blend/1.0.0
    data_replication_factor: 0
  core:
    scheduler:
      cover:
        message_frequency_per_round: 1.0
        intervals_for_safety_buffer: 100
      delayer:
        maximum_release_delay_in_rounds: 1
    minimum_messages_coefficient: 1
    normalization_constant: 1.03
    activity_threshold_sensitivity: 1
network:
  kademlia_protocol_name: /logos-blockchain-devnet-0.1.3-rc.6/kad/1.0.0
  identify_protocol_name: /logos-blockchain-devnet-0.1.3-rc.6/identify/1.0.0
  chain_sync_protocol_name: /logos-blockchain-devnet-0.1.3-rc.6/chainsync/1.0.0
cryptarchia:
  epoch_config:
    epoch_stake_distribution_stabilization: 3
    epoch_period_nonce_buffer: 3
    epoch_period_nonce_stabilization: 4
  security_param: 50
  slot_activation_coeff:
    numerator: 1
    denominator: 30
  learning_rate: 0.5
  sdp_config:
    service_params:
      BN:
        lock_period: 10
        inactivity_period: 1
        retention_period: 1
        timestamp: 0
    min_stake:
      threshold: 1
      timestamp: 0
  gossipsub_protocol: /logos-blockchain-devnet-0.1.3-rc.6/cryptarchia/1.0.0
  genesis_block:
    header:
      version: Bedrock
      parent_block: '0000000000000000000000000000000000000000000000000000000000000000'
      slot: 0
      block_root: c533b77bc7ef8263cee93391f0f191d774e067b263b8fae760a308ae33257186
      proof_of_leadership:
        proof: '0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000'
        entropy_contribution: '0000000000000000000000000000000000000000000000000000000000000000'
        leader_key: '0000000000000000000000000000000000000000000000000000000000000000'
        voucher_cm: '0000000000000000000000000000000000000000000000000000000000000000'
    signature: '00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000'
    transactions:
    - mantle_tx:
        ops:
        - opcode: 0
          payload:
            inputs: []
            outputs:
            - value: 100000
              pk: 31e61fd6063b4151c79cbb840bb4532ee2660bc5a6a03d518049ec1bf1c4052d
            - value: 1
              pk: fa868e23c670a426fd467eab8e75371cfb93d0ccb8aa679a63032f8b39bab918
            - value: 100
              pk: f816ce1f1f6bca3d2fbf2404e8b75223c1bf604760fc290311eaae89ceebe32f
            - value: 100000
              pk: 96dd5ca55dcdc3a67779a381ee83df1667e6aa7b89e294c426bb4b6137a75d22
            - value: 1
              pk: f635bffd8fb91f61a0f3de2e24372d59941c87066a066d178b32f726dcd5492b
            - value: 100
              pk: 5c2e517fe8635035d50b955300e9a7e8eedffc0689433413b422440bd7cd3b01
            - value: 100000
              pk: db945a4fd41a0e352129c166bb9116d1b60df8c3dffe330904a4123d96182b16
            - value: 1
              pk: 8f5b4ad7baee2798100b6b9b91ebb222316550a2a66a9f8a63052ac64cecf426
            - value: 100
              pk: ec10a7f6d41d9d845768f107543ebcae67220050d15abdf74a2b8845c8163e22
            - value: 100000
              pk: 9b017f5b318cfcfdb40e8d7bbddf6df4791eb80a43c4eba5a8b581150abbd906
            - value: 1
              pk: 6e8514be6224867f39d4117a5cc6ed599a7d0799dfcf7e365a030321e39b0218
            - value: 100
              pk: a16fb0d5a34579e4833195147f32c0dec63ede9a0f1fb2af62397af0766ae715
            - value: 18446744073709151211
              pk: 3b3e025165f51ee75d3f94c61728337998462a31f0279a7906922ff281cc251e
        - opcode: 17
          payload:
            channel_id: '0000000000000000000000000000000000000000000000000000000000000000'
            inscription: 4c0000000000000070726f636573735f73746172745f6e6f6e63653d313861666234653930653437663464372d30303030303030312c20746573745f656e74726f70793d36313434386133623739636634326131faee066a000000000000000000000000000000000000000000000000000000000000000000000000
            parent: '0000000000000000000000000000000000000000000000000000000000000000'
            signer: '0000000000000000000000000000000000000000000000000000000000000000'
        - opcode: 32
          payload:
            service_type: BN
            locators:
            - /ip4/65.108.203.235/udp/3400/quic-v1
            provider_id: b78a84df2d01094d6731c556a3a0ca32db2f818572dae1986f1eb84290efa2e8
            zk_id: 52103b53ac3f6e0dcd29c4f4e94c7d8a6c6d0841c855d7a6ad312406e2786310
            locked_note_id: cc63065afe5cddf032188c8a7f67c9b7ab7abc00eef0fda9c3231cbec4db4b0e
        - opcode: 32
          payload:
            service_type: BN
            locators:
            - /ip4/65.108.203.235/udp/3401/quic-v1
            provider_id: 42d3d35e2a1b5bfa4e79b8f7fdf151fff6e56739f05c97d77971953267cbc497
            zk_id: 6f62515b60069b3ae245d3cba6e411a9c1f2bf03667bde96531d3479c0e4f815
            locked_note_id: 62a00bc780483882611151e9b26e973301a88c00f84b0fde300fecddc6cf810a
        - opcode: 32
          payload:
            service_type: BN
            locators:
            - /ip4/65.108.203.235/udp/3402/quic-v1
            provider_id: 85c17d2497723b152a040429be4cffa338ae855ab028044f2962d84c701dd1d1
            zk_id: b81dd5af910ce9b31f1460a62d4a668a876c36d958c2586306d065ece55b5e11
            locked_note_id: c378dca2df3954050aa410643d21a509ef1300422db0399e1802f6ed237c1112
        - opcode: 32
          payload:
            service_type: BN
            locators:
            - /ip4/65.108.203.235/udp/3403/quic-v1
            provider_id: 230b98193bd53d4180860466b70f3f42560dc568eeb4f54f99b048b9d95416f0
            zk_id: a05a1142bd6798df695422d1cb5d3e6d652d67ba43a8f4c5883054d6017b0d10
            locked_note_id: cc6e5fbe41b65420e55527d11f3c31e384a397d17edbb65af7a95de01ec64210
      ops_proofs:
      - !ZkSig
        pi_a: '0000000000000000000000000000000000000000000000000000000000000000'
        pi_b: '00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000'
        pi_c: '0000000000000000000000000000000000000000000000000000000000000000'
      - !Ed25519Sig '00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000'
      - !ZkAndEd25519Sigs
        zk_sig:
          pi_a: 942ef4cb23c405456c78aad035eb7bd8b9260e3b722a414b3b56bdaa23b49505
          pi_b: 3c9c439afadc115b5d4bdb085a9daf51782c7c3f1b10df810de0a2fe38ecc91c70c1341c1ebe7f52f0cbb1e46ac90c0f6b890540336f94c55e22f181948a3501
          pi_c: 8f7d79fc6f039bc701ade52fd770968217fdfb313dea37104a4d9a4836954a04
        ed25519_sig: 497295f892eb841fbb12eeea2fba94838f214c3fe5c3b783079e06e72274abec6702304d9245622daf1f5ffbe724fb22cfb80a01f1d310bb5f94e847d303f402
      - !ZkAndEd25519Sigs
        zk_sig:
          pi_a: 58eb541482ec88e31359b8e904a14a1dd84721e7760acc6133f31a1121a59a25
          pi_b: a0dbe18f1c76a0071d21f08610a98554de820755f1143c83f02304fa2a11b81ead36d282a05ae6c67b415de7f84b2d1e56ed6ca6b2780a697ec582bc48e90e07
          pi_c: 9f07ece5dfd43af2a175b85c3c65d8314007e06066e0aadb8da529dc33532f21
        ed25519_sig: 87f465cd013ab02aabd44eeaa92ff853a142045e71821eb3369bcc7d973ed3a4df849a9e4add1c8cada89b5f2cbd2a149ea02964b07f96200468d7e040b24701
      - !ZkAndEd25519Sigs
        zk_sig:
          pi_a: f4ba06162309938aa1033953cd83dc90d354ace828d6e2944177c0ce16a5aa27
          pi_b: 9a647e10332ff5a5accec6703cd2a9018332b8e71042538c9800cdeb327e2d21413b0cb414c25848106cae14dfa5716c176bf32044aa84628229b1e88722b62e
          pi_c: 440d97c4348704c72798a41829342abde59bf7a7d3149e1075db510bda636426
        ed25519_sig: d98fc20c2436b533f02d040f9bc02b4a6246fa3e5b4fa0af9aa88793e28237877681fe201f232860ff3b872c48161ec878bd83a8c22250d13114d7cd49438504
      - !ZkAndEd25519Sigs
        zk_sig:
          pi_a: 71e288c9e2460d39963f4c9c98c152432524c37eeb04822634e60e3854fdac98
          pi_b: 0df8ebdc268a44e5f3fb5c0338f4c1b9165049ed7e2b5ff674a3432a98597d2ae31af1d5d860eeec28343655c677af5bc0a5cc0739ad4ff8ade16cbb22c2f395
          pi_c: d2e63249e1316317f579b6fdb07d65bcd7556d2af1231f977f7e0a79537d3303
        ed25519_sig: b5828b032967205e8cb160c3ec940d903ebfa0e3c8429454dde9384c6540a57c1fb0767b9d9397a07d9f64b4c0d3f63d4b63525c253709e926970ac3758e5d0e
  faucet_pk: 3b3e025165f51ee75d3f94c61728337998462a31f0279a7906922ff281cc251e
time:
  slot_duration: '1.000000000'
mempool:
  pubsub_topic: /logos-blockchain-devnet-0.1.3-rc.6/mempool/1.0.0
";
