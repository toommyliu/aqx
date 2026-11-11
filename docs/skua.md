# Run Skua scripts

aqx runs most [Skua](https://github.com/auqw/Skua) scripts unchanged. A worker built with .NET compiles and runs them. aqx keeps the Skua script library and its settings in one folder, `~/aqx/skua`.

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
3. Download the script library. In the web UI, press Run, switch to the Skua tab, then press Update. From a shell, run `aqx skua update`.

	`aqx skua update` downloads the `Skua` branch of [auqw/Scripts](https://github.com/auqw/Scripts), about 3 MB, into `~/aqx/skua/Scripts`. Then it builds the worker. The first build takes about a minute, because .NET downloads the worker's packages.

aqx contacts GitHub only when you press Update or run `aqx skua update`. Scripts can still use the network on their own, as they do under Skua. For example, CoreBots downloads its quest list when its copy is more than two weeks old.

## Run a script from the library

In the web UI, press Run on an account, or on the accounts you ticked, and switch to the Skua tab. The list shows the scripts in the library, each with its description and tags. The list leaves out files and folders whose names start with `Core`, such as `CoreBots.cs`, because they hold shared code and aren't meant to run. `aqx skua list` leaves them out too. The search box searches names, folders, descriptions, and tags. A script matches when it contains every word you type. A word that starts with `#` searches only tags. If a tag has exactly that name, the word matches only that tag, so `#rep` skips scripts tagged `reputation`. Otherwise the word matches every tag that starts with it, so `#mer` already finds the scripts tagged `merge`. `#merge legion` finds the scripts tagged `merge` that mention Legion. Pick a script to run it on those accounts.

To run a library script each time an account logs in, set the account's `script` in `aqx.toml`. Paths in `aqx.toml` are relative to the file, so with the file in `~/aqx` the path starts with `skua/Scripts`:

```toml
[[account]]
user = "player1"
password = "change-me"
script = "skua/Scripts/Evil/NSoD/VoidAuras/0SmartVoidAuras.cs"
```

From a shell, `aqx skua list` prints the full path and the name of each script that matches the words you give it, including `#tag` words. Pass that path to `aqx run`:

```sh
aqx skua list smart void
aqx run -a player1 ~/aqx/skua/Scripts/Evil/NSoD/VoidAuras/0SmartVoidAuras.cs
```

## Change script options

In the web UI, press Run, and press the options button next to a Skua script on the Skua tab. For a Skua script outside the library, the Files tab has the same button next to each `.cs` file. The **CoreBots** tab edits the CoreBots options of that account. The **Script** tab edits the options of the script, and every account that runs the script shares them. You can change options only while the account is offline or idle.

To give one account its own value, set it in that account's `args` in `aqx.toml`, or under Script options in the config editor. It applies on top of the shared options. From a shell, pass options to `aqx run` as `--arg Name=value`. For an option outside the `Options` group, write `--arg Group:Name=value`, and in `args` write `"Group:Name" = value`.

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

In the web UI, press Run, switch to the Skua tab, and press Update. From a shell, run `aqx skua update`. aqx downloads the library only when it changed upstream.

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
