# dxclrecorder

A recorder for spots published by DX cluster servers. The recorder is able to connect to multiple servers in parallel. All received spots will be parsed into JSON and pushed to the configured output channels.

The application configuration happens within the `dxclrecorder.json` file. The filename can be adjusted by a commandline option, see `--help`.


## Supported DX-Clusters

* DXSpider
* AR-Cluster
* CC Cluster
* Reverse Beacon Network (RBN)


## Configuration

### Connection

The `servers` array contains at least one set of connection parameters. If the connection to the server got lost, the option to `reconnect` can be enabled. The parameters `retries` and `backoff` define the number of connection retries before quitting and the delay in seconds between each attempt.

### Output

The parsed spots will be pushed to the enabled output channels. Next to the `console` (`stdout`) output directly writing to a file is supported. The options `rotate` and `compress` activate the daily file rotation and compression of the rotated file.

### Logging

The log output will be pushed to the `console` (`stdout`) and/or directly to a file.

### Filter

Filters for the `type` and the `band` can be applied to each received spot, whereby filtering the band only applies for spots of the type `DX`. Each type of spot can be allowed or filtered out. The band filter array contains all bands that shall be kept. Spots with different bands will be ignored. Spots, that do not match any band will also be ignored. In case of an empty filter array all spots will be kept.


## Build and Run

To build the application run `cargo build [--release]`. Before running the recorder adjust the configuration file to your needs. For a simple test just add a connection string to your preferred cluster server. Afterwards run the application. Now both the log information and the cluster spots will be printed to the console.