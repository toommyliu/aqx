# Run Skua scripts

The `skua/host` branch runs Skua's C# scripts (`.cs`) on aqx. A .NET program, the Skua host, compiles each script against Skua's interfaces and sends its game calls to aqx. The Skua host is experimental. Many Skua calls aren't supported yet, and some behave differently than in Skua.

## Install aqx with the Skua host

You need the C compiler and Rust from [Get started](getting-started.md#install-the-build-tools), and the [.NET 10 SDK](https://dotnet.microsoft.com/download/dotnet/10.0).

aqx finds the Skua host inside the folder that aqx was built from. So `cargo install --git` doesn't work here. Build aqx and the Skua host from one clone, and keep that clone.

1. Clone the branch with its submodules. The submodules are Skua and Skua's script repo.

   ```
   git clone --branch skua/host --recurse-submodules --shallow-submodules https://github.com/toommyliu/aqx
   cd aqx
   ```

2. Build the Skua host:

   ```
   dotnet publish skua/Host/Host.csproj -c Release -r osx-arm64 --self-contained -o skua/Host/bin/publish
   ```

   `osx-arm64` is for an Apple Silicon Mac. On another system, replace it with that system's runtime ID: `osx-x64` for an Intel Mac, `linux-x64`, or `linux-arm64`.

3. Install aqx from the clone:

   ```
   cargo install --locked --path crates/aqx
   ```

Don't move or delete the clone. aqx runs the Skua host from `skua/Host/bin/publish` in the clone. To use a Skua host in another place, set `AQX_SKUA` to its path.

To upgrade, run `git pull` and `git submodule update` in the clone. Then do steps 2 and 3 again.

## Run a script

aqx sends every `.cs` file to the Skua host. Run it the same way as a `.ts` script: with `aqx run`, `script` in `aqx.toml`, the terminal UI, or `aqx ctl`.

```
aqx run skua/vendor/Scripts/Farm/BuyScrolls.cs
```

A script loads `CoreBots.cs` and its other includes from the nearest folder above it that holds `CoreBots.cs`. `skua/vendor/Scripts` is a copy of Skua's script repo, so its scripts work as they are. A script outside such a folder uses `skua/Scripts` in the aqx folder, `~/aqx/skua/Scripts` by default. aqx keeps the rest of Skua's data there too, such as each script's saved options.

To set a script option, pass `--arg Name=value`. For an option in a group, pass `--arg Group:Name=value`. aqx loads the Skua options file of the script first, and `--arg` overrides it.

## Check a script without logging in

`aqx check` works only on `.ts` files. For `.cs` files, run the Skua host directly:

```
skua/Host/bin/publish/aqx-skua check --scripts skua/vendor/Scripts skua/vendor/Scripts/Farm/BuyScrolls.cs
```

Without file names, `check` compiles every script in the `--scripts` folder.

To list the Skua calls that the scripts use and aqx doesn't support, run `aqx-skua gaps --scripts skua/vendor/Scripts`. The most used calls come first.
