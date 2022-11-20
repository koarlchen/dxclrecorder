# dxclrecorder

A recorder for spots published by DX cluster servers. The recorder is able to connect to multiple servers in parallel. All received spots will be parsed into JSON and pushed to the configured output channels. Next to console output directly writing to a file is supported. It can be configured that the file will be rotated daily relative to UTC timezone to match the timestamps being part of the spots.

Furthermore filters for the band and type of spot can be applied. The array of bands behaves like a whitelist for spots on that band.

The application configuration happens within the `dxclrecorder.json`. The filename can be adjusted by a commandline option.


## Supported DX-Clusters

* DXSpider
* AR-Cluster
* CC Cluster
* Reverse Beacon Network


## Build and Run

To build the application run `cargo build [--release]`. The application itself is not os dependent.

Before running the recorder adjust the configuration file to your needs. For a simple test just add a connection string to your preferred cluster server. Afterwards run the application. Now both the log information and the cluster spots will be printed to the console.