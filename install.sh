#!/bin/sh

cargo build -r
sudo cp /target/release/ask_rs /bin/ask
echo "Program installed! Call with ask"
