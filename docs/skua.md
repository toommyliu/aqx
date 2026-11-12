# Run Skua scripts

aqx runs most [Skua](https://github.com/auqw/Skua) scripts unchanged using a .NET worker. The script library and its settings live in `~/aqx/skua`.

<img width="2930" height="1820" alt="skua" src="https://github.com/user-attachments/assets/3a4928bd-2762-4ae1-afe0-23152d2abfe0" />


## Set up Skua support

1. Install the [.NET 10 SDK](https://dotnet.microsoft.com/download).
2. Clone aqx, fetch the Skua submodule, and install aqx from the clone:

	```sh
	git clone https://github.com/toommyliu/aqx.git
	cd aqx
	git submodule update --init --depth 1 skua/vendor/Skua
	cargo install --path crates/aqx
	```

	The installed `aqx` builds the Skua worker from this clone. If you move or delete the clone, Skua scripts stop working until you install aqx again from a clone.
3. Download the script library. In the web UI, press Run, switch to the Skua tab, then press Update. In a terminal, run `aqx skua update`.

	`aqx skua update` downloads the `Skua` branch of [auqw/Scripts](https://github.com/auqw/Scripts), about 3 MB, into `~/aqx/skua/Scripts`. Then it builds the worker. The first build takes about a minute, because .NET downloads the worker's packages.

aqx contacts GitHub only when you press Update or run `aqx skua update`. Scripts can still use the network on their own, as they do under Skua. For example, CoreBots downloads its quest list when its copy is more than two weeks old.

## Run a script from the library

The web UI and `aqx skua list` search script names, folders, descriptions, and tags. Prefix a word with `#` to search tags. Files and folders whose names start with `Core` are excluded because they contain shared code.

To run a library script each time an account logs in, set the account's `script` in `aqx.toml`. Paths in `aqx.toml` are relative to the file, so with the file in `~/aqx` the path starts with `skua/Scripts`:

```toml
[[account]]
user = "player1"
password = "change-me"
script = "skua/Scripts/Evil/NSoD/VoidAuras/0SmartVoidAuras.cs"
```

In a terminal, `aqx skua list` prints the full path and the name of each script that matches the words you give it, including `#tag` words. Pass that path to `aqx run`:

```sh
aqx skua list smart void
aqx run -a player1 ~/aqx/skua/Scripts/Evil/NSoD/VoidAuras/0SmartVoidAuras.cs
```

## Change script options

CoreBots options belong to an account. Script options are shared by every account that runs the script. You can change them only while the account is offline or idle.

To give one account its own value, set it in that account's `args` in `aqx.toml`, or under Script options in the config editor. It applies on top of the shared options. In a terminal, pass options to `aqx run` as `--arg Name=value`. For an option outside the `Options` group, write `--arg Group:Name=value`, and in `args` write `"Group:Name" = value`.

A queued Skua script reads the shared options when it starts, not when you queue it. To give one run its own values, press the options button beside it in the account's Up next list. Those values apply to that run only, and you can change them while another script runs, because they don't touch the shared options.

## Write your own Skua script

Keep your scripts in any folder outside `~/aqx/skua/Scripts`. Their `//cs_include Scripts/...` lines read files from the library:

```cs
//cs_include Scripts/CoreBots.cs
using Skua.Core.Interfaces;

public class Gold
{
	public CoreBots Core => CoreBots.Instance;

	public void ScriptMain(IScriptInterface bot)
	{
		Core.Logger("hi");
	}
}
```

To compile the script without logging in, run `aqx check Gold.cs`.

If a folder above the script has its own `CoreBots.cs`, aqx reads the includes from that folder instead of the library. A clone of auqw/Scripts works this way.

## Update the library

In the web UI, press Run, switch to the Skua tab, and press Update. In a terminal, run `aqx skua update`. aqx downloads the library only when it changed upstream.

An update replaces the whole `~/aqx/skua/Scripts` folder, so aqx discards any edits you made inside it. If the download fails, aqx keeps the current library. Scripts that are running keep running, and the next start uses the new library.

## Update aqx

Pull the clone, update the submodule, and install aqx again:

```sh
git pull
git submodule update --init --depth 1 skua/vendor/Skua
cargo install --path crates/aqx
```

If the worker sources changed, the next Skua script start rebuilds the worker, and the account log shows `building the Skua worker`.

## Find the Skua folder

Run `aqx skua path` to print the folder. To move it, set `AQX_HOME`, which moves the rest of `~/aqx` too.

The folder holds the library in `Scripts`, CoreBots and script options in `options`, and compiled scripts in `cache`. aqx doesn't read the data folder of Skua itself. To keep options that you set in Skua, copy its `options` folder into `~/aqx/skua`.
