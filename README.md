# DX Cluster Recorder

A recorder for spots published by DX cluster servers.
The recorder is able to connect to multiple servers in parallel.
All received spots will be parsed into JSON and pushed to the configured output channels.

The clients may be reconnected to the cluster servers if configured.
If a client finally wasn't able to establish a connection, the program will exit.
In case you wish that the program should not exit upon a single connection error you may run multiple instances in parallel.


## Usage

In addition to standard options like `--version` and `--help` verbose output can be enabled.
Furthermore, the name of the configuration file can be adjusted.

```
A recorder for spots published by DX cluster servers

Usage: dxclrecorder [OPTIONS]

Options:
  -c, --config <CONFIG>  Sets the configuration file to use [default: dxclrecorder.json]
  -v, --verbose          Enable verbose log output
  -h, --help             Print help
  -V, --version          Print version
```

## Configuration

The application configuration happens within the `dxclrecorder.json` file.
The filename can be adjusted by a commandline option, see `--help`.

- `connection`:
    
    The `servers` array contains at least one set of connection parameters.
    If the connection to the server got lost, the option to `reconnect` can be enabled.
    The parameters `retries` and `backoff` define the number of connection retries before quitting and the delay in seconds between each attempt.

- `output`:

    The parsed spots will be pushed to the enabled output channels.
    Next to the `console` (`stdout`) output directly writing to a file is supported.
    The options `rotate` and `compress` activate the daily file rotation and compression of the rotated file.

- `logging`:

    The log output will be pushed to the `console` (`stdout`) and/or directly to a file.

- `filter`:

    Filters for the `type` and the `band` can be applied to each received spot, whereby filtering the band only applies for spots of the type `DX`.
    Each type of spot can be allowed or filtered out.
    The band filter array contains all bands that shall be kept.
    Spots with different bands will be ignored.
    Spots, that do not match any band will also be ignored.
    In case of an empty filter array all spots will be kept.
    The identifier for each band is defined [here](https://docs.rs/crate/hambands/1/source/src/band/mod.rs).


## Supported DX Clusters

The following four cluster server implementations are more or less tested and can be used for recording.
Somewhat invalid spots are filtered out but logged at `DEBUG` level.
Reasons may be received strings that are not in the format of a spot or spots where the frequency could not be matched to any band.
Rarely spots with invalid utf-8 encoding can be found, but those are dropped at an earlier stage and are therefore not logged.


* DXSpider
* AR-Cluster
* CC Cluster
* Reverse Beacon Network (RBN)


## Build and Run

To build the application run `cargo build [--release]`.
Before running the recorder, adjust the configuration file to your needs.
For a simple test just add a connection string to your preferred cluster server and run the app.
Now both the log information and the cluster spots will be printed to the console.