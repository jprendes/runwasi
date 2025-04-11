# Hyperlight shim

## Clone the repo
```
git clone https://github.com/jprendes/runwasi.git
git checkout hyperlight-shim
```

## Build the shim
```
cargo build -p containerd-shim-hyperlight
```

## Generate an image
```
cargo run -p oci-tar-builder -- \
    --repo ghcr.io/containerd/runwasi --name simpleguest --tag latest \
    -M "application/hyperlight" \
    -m /path/to/simpleguest \
    -o simpleguest-oci.tar
```

## Load the image in containerd
```
sudo ctr i import --local --all-platforms simpleguest-oci.tar
```

## Run the image
```
sudo ctr run \
    --rm \
    --runtime=$PWD/target/debug/containerd-shim-hyperlight-v1 \
    ghcr.io/containerd/runwasi/simpleguest:latest \
    testhyperlight
```
