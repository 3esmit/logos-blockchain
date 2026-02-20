# Logos SQLite Zone Sequencer and Indexer Demo

This directory contains a reference implementation of a Sovereign Zone solution using the Logos Blockchain as a simple database server.

## System Architecture

In this demo, the sequencer acts as the primary maintainer of a "Menu" database, with DB updates published to the Logos Blockchain. Other parties, known as indexers, can follow these updates to reconstruct the same database locally.

1. **Sequencer**: Is the central authority maintaining the Menu database. Users can interact with DB (reading and writing) via the command line. Database updates are posted as inscriptions to the Logos Blockchain. 
2. **Logos Blockchain**: Acts as the immutable ledger from which the database can be reconstructed by any interested party.
3. **Indexer**: Watches the sequencer's channel for updates. It pulls data from these inscriptions as they come in and reconstructs the Menu database locally. Users can interact with DB (read only) via the command line.

---

## Project Structure

Each component is a standalone service that can be run independently or via Docker.

| Component | Directory | Responsibility |
| --- | --- | --- |
| **Sequencer** | `sequencer/` | Maintaining database and posting updates. |
| **Indexer** | `indexer/` | Channel monitoring and reconstructing database based on updates. |

---

## Getting Started

### Prerequisites

* **Rust**: For building the Sequencer and Indexer binaries, if running the helper script.
* **Logos Testnet Credentials**: If connecting to the public testnet, you need basic auth credentials (Username/Password). Contact the team via Discord to obtain these.

### 1. Configuration

Copy the example environment file and fill in your information.

```bash
cp testnet/sqlite-zone-demo/.env.example-local testnet/sqlite-zone-demo/.env-local

```
You will need access to a running **Logos Node**. If you are running one locally, ensure the `SEQUENCER_NODE_ENDPOINT` and `INDEXER_NODE_ENDPOINT` in your `.env-local` both point to your local node.


For the built-in execution method, you can also provide these fields via command line arguments (see below).

### 2. Running the Sequencer

#### 2a. Using the Built-In Runner

The Logos Blockchain Node binary comes with a built-in argument, allowing you to run the SQLite Zone Sequencer:

```bash
./logos-blockchain-node sqlite-sequencer [arguments]

```

This command should usually work without any additional arguments, relying on default values. The full list of arguments is provided below:

| Argument with Example | Description|
| --- | --- |
| `--node-url http://localhost:8080` | Logos blockchain node HTTP endpoint. |
| `--db-path ./database.db` | Path to the SQLite database file. |
| `--key-path ./sequencer.key` | Path to the signing key file (created if it doesn't exist). |
| `--node-auth-username username` | Basic auth username for node endpoint. |
| `--node-auth-password password` | Basic auth password for node endpoint. |
| `--queue-file ./queue.txt` | Path to the queue file for pending SQL statements. |
| `--checkpoint-path ./sequencer.checkpoint` | Path to the checkpoint file for crash recovery. |


#### 2b. Using the Local Runner

You can also run the following file to execute the sequencer directly: `testnet/sqlite-zone-demo/run-local.sh`.

The script automates building binaries, managing data directories, and linking environment variables between services.

```bash
# Change to correct path
cd testnet/sqlite-zone-demo

# Usage
./run-local.sh <service> --env-file <path-to-env> [--clean]

# Run only a specific component
./run-local.sh sequencer --env-file .env-local

# Start fresh (deletes local databases/keys)
./run-local.sh sequencer --env-file .env-local --clean

```

Running this script should allow you to enter SQL queries into the command line.

### 3. Using the Sequencer

When the Sequencer starts up without a sequencer key from a previous run, it will generate a new key with an associated Channel ID. Make sure to copy this Channel ID as it is required to run the Indexer.

```bash
Channel ID: beefbeefbeef0000000000000000000000000000000000000000000000000011
```

Any SQL query can be submitted to modify or read from the database.

```bash
Type SQL queries followed by ENTER
Type 'q' or CTRL+C then ENTER to exit.
> INSERT INTO menu (name, data) VALUES ("Cookie", "With extra chocolate chips")
```

If a SELECT query is sent, the response will include the Menu items that match the given pattern.

```bash
> SELECT * FROM menu
ID: 1 | Name: Cookie | Data: With extra chocolate chips
(1 row(s))
```

### 4. Running the Indexer

#### 2a. Using the Built-In Runner

The Logos Blockchain Node binary comes with a built-in argument, allowing you to run the SQLite Zone Indexer in a new terminal window:

```bash
./logos-blockchain-node sqlite-indexer --channel-id beefbeefbeef0000000000000000000000000000000000000000000000000011 [other arguments]

```

The Channel ID obtained from the Sequencer must be provided to the Indexer so it can monitor the correct channel. This can be done via the command line argument as above, or by setting the CHANNEL_ID environment variable.

The full list of arguments is provided below:

| Argument with Example | Description|
| --- | --- |
| `--node-url http://localhost:8080` | Logos blockchain node HTTP endpoint. |
| `--db-path ./database.db` | Path to the SQLite database file. |
| `--key-path ./sequencer.key` | Path to the signing key file (created if it doesn't exist). |
| `--node-auth-username username` | Basic auth username for node endpoint. |
| `--node-auth-password password` | Basic auth password for node endpoint. |
| `--channel-id beefbeefbeef0000000000000000000000000000000000000000000000000011` | Channel ID to index. |

#### 2b. Using the Local Runner

You can also run the following file to execute the sequencer directly: `testnet/sqlite-zone-demo/run-local.sh`.

The script automates building binaries, managing data directories, and linking environment variables between services.

```bash
# Change to correct path
cd testnet/sqlite-zone-demo

# Usage
./run-local.sh <service> --env-file <path-to-env> [--clean]

# Run only a specific component
./run-local.sh indexer --env-file .env-local

# Start fresh (deletes local databases/keys)
./run-local.sh indexer --env-file .env-local --clean
```

### 5. Using the Indexer

Make sure to wait for the "Applied X statement(s)" info message from the Indexer to make sure it received the latest updates before querying. This may take a few minutes, so please be patient.

The Indexer will only permit SELECT queries to be made to its local database.

```bash
Type SQL queries followed by ENTER
Type 'q' or CTRL+C then ENTER to exit.

> SELECT * FROM menu
ID: 1 | Name: one | Data: two
(1 row(s))
```