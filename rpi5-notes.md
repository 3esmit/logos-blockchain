# How to build on Raspberry Pi 5

1. Make sure Docker is installed and running
1. If you are using RPi 5 8GB, make sure your swap is big enough
   ```
   cat /sys/block/zram0/disksize
   ```

   if it shows `2147483648` you might want to increase it

   ```
   sudo apt install zram-tools
   sudo swapoff /dev/zram0
   sudo zramctl --reset /dev/zram0
   sudo zramctl --find --size 8G --algorithm lzo-rle
   sudo mkswap /dev/zram0
   sudo swapon /dev/zram0
   ```

   Check if swap is properly enabled

   ```
   swapon --show
   free -h
   ```
1. Install potentially missing deps
   ```
   sudo apt-get install libclang-dev clang pkg-config libssl-dev
   ```

1. Build circuits from https://github.com/logos-blockchain/logos-blockchain-circuits/pull/8
   ```
   git clone https://github.com/logos-blockchain/logos-blockchain-circuits
   cd logos-blockchain-circuits/
   ./scripts/docker-build.sh
   ```

   If it succeeds, it should result in folder `~/.nomos-circuits/` created with circuits present there
1. Run the `setup-nomos-circuits.sh`
   ```
   ./scripts/setup-nomos-circuits.sh
   ```

   If it says installation already exists, hit N to **cancel**.
1. Install [Rust](https://www.rust-lang.org/tools/install) and follow the [README](https://github.com/logos-blockchain/logos-blockchain/blob/master/README.md#running-logos-blockchain-node-with-integration-test) to build and run Logos Blockchain Node locally and execute tests
  
   You should see blocks being produced - YAY!