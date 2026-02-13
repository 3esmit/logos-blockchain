# Logos SQLite Zone Sequencer and Indexer Demo

This directory contains a reference implementation of a Sovereign Zone solution using the Logos Blockchain as a simple database server.

## System Architecture

In this demo, the sequencer acts as the primary maintainer of a "Menu" database, with DB updates published to the Logos Blockchain. Other parties, known as indexers, can follow these updates to reconstruct the same database locally.

1. **Sequencer**: Is the central authority maintaining the Menu database. Users can interact with DB (reading and writing) via API endpoints it exposes. Database updates are posted as inscriptions to the Logos Blockchain. 
2. **Logos Blockchain**: Acts as the immutable ledger from which the database can be reconstructed by any interested party.
3. **Indexer**: Watches the sequencer's channel for updates. It pulls data from these inscriptions as they come in and reconstructs the Menu database locally. Users can interact with DB (read only) via exposed API endpoints.

---

## Project Structure

Each component is a standalone service that can be run independently or via Docker.

| Component | Directory | Responsibility |
| --- | --- | --- |
| **Sequencer** | `sequencer/` | Maintaining database and posting updates. |
| **Indexer** | `indexer/` | Channel monitoring and reconstructing database based on updates. |

---

## Getting Started (Local Run)

We provide a unified helper script: `testnet/sqlite-zone-demo/run-local.sh`.

### Prerequisites

* **Rust**: For building the Sequencer and Archiver binaries.
* **Bun**: For running the frontend development server.
* **OpenSSL**: For generating unique Channel IDs.
* **Logos Testnet Credentials**: If connecting to the public testnet, you need basic auth credentials (Username/Password). Contact the team via Discord to obtain these.

### 1. Configuration

Copy the example environment file and fill in your credentials.

```bash
cp testnet/sqlite-zone-demo/.env.example-local testnet/sqlite-zone-demo/.env-local

```

You will need access to a running **Logos Node**. If you are running one locally, ensure the `SEQUENCER_NODE_ENDPOINT` and `INDEXER_NODE_ENDPOINT` in your `.env-local` both point to your local node.

### 2. Using the Local Runner

The script automates building binaries, managing data directories, and linking environment variables between services.

```bash
# Change to correct path
cd testnet/sqlite-zone-demo

# Usage
./run-local.sh <service> --env-file <path-to-env> [--clean]

# 1. Run the entire stack (Sequencer + Indexer)
./run-local.sh all --env-file .env-local

# 2. Run only a specific component
./run-local.sh sequencer --env-file .env-local

# 3. Start fresh (deletes local databases/keys)
./run-local.sh all --env-file .env-local --clean

```

### 3. Using the Sequencer API

#### Query (POST)

Any SQL query can be submitted to modify or read from the database.

```bash
curl -X POST $SEQUENCER_LISTEN_ADDR/query   -H "Content-Type: application/json"   -d '{"query": "INSERT INTO menu (name, data) VALUES (\"Cookie\", \"With extra chocolate chips\")"}'
```

If a SELECT query is sent, the response will include the Menu items that match the given pattern.

```bash
curl -X POST $SEQUENCER_LISTEN_ADDR/query   -H "Content-Type: application/json"   -d '{"query": "SELECT * FROM menu"}'

> {"dishes":[{"id":1,"name":"Cookie","data":"With extra chocolate chips"}]}
```

#### Health (GET)

A basic health check. Returns OK if all is well.

```bash
curl -X GET $SEQUENCER_LISTEN_ADDR/health
> OK
```

### 4. Using the Indexer API
Make sure to wait for the 

#### Query (GET)

This will only permit SELECT queries to be made to the Indexer's local database.

```bash
curl -X GET $INDEXER_LISTEN_ADDR/query   -H "Content-Type: application/json"   -d '{"query": "SELECT * FROM menu"}'

> {"dishes":[{"id":1,"name":"Cookie","data":"With extra chocolate chips"}]}
```

#### Health (GET)

A basic health check. Returns OK if all is well.

```bash
curl -X GET $INDEXER_LISTEN_ADDR/health
> OK
```