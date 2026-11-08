# Run Skua scripts

mira runs most [Skua](https://github.com/auqw/Skua) scripts unchanged. A worker built with .NET compiles and runs them. mira keeps the Skua script library and its settings in one folder, `~/mira/skua`.

## Set up Skua support

1. Install the [.NET 10 SDK](https://dotnet.microsoft.com/download).
2. Clone mira, fetch the Skua submodule, and install mira from the clone:

	```sh
	git clone https://github.com/toommyliu/mira.git
	cd mira
	git submodule update --init --depth 1 skua/vendor/Skua
	cargo install --path crates/mira
	```

	The installed `mira` builds the Skua worker from this clone. If you move or delete the clone, Skua scripts stop working until you install mira again from a clone.
3. Download the script library. In the TUI, press `S`, then `tab` to the Skua tab, then `u`. From a shell, run `mira skua update`.

	`mira skua update` downloads the `Skua` branch of [auqw/Scripts](https://github.com/auqw/Scripts), about 3 MB, into `~/mira/skua/Scripts`. Then it builds the worker. The first build takes about a minute, because .NET downloads the worker's packages.

mira contacts GitHub only when you press `u` or run `mira skua update`. Scripts can still use the network on their own, as they do under Skua. For example, CoreBots downloads its quest list when its copy is more than two weeks old.

## Run a script from the library

In the TUI, mark the accounts, press `S`, and `tab` to the Skua tab. The list shows the scripts in the library, and the bottom lines show the description and the tags of the selected script. The list leaves out files and folders whose names start with `Core`, such as `CoreBots.cs`, because they hold shared code and aren't meant to run. `mira skua list` leaves them out too. Press `/` to search names, folders, descriptions, and tags. A script matches when it contains every word you type. A word that starts with `#` searches only tags. If a tag has exactly that name, the word matches only that tag, so `#rep` skips scripts tagged `reputation`. Otherwise the word matches every tag that starts with it, so `#mer` already finds the scripts tagged `merge`. `#merge legion` finds the scripts tagged `merge` that mention Legion. Press `enter` to run the selected script on the marked accounts, or on the selected account when no account is marked.

To run a library script each time an account logs in, set the account's `script` in `mira.toml`. Paths in `mira.toml` are relative to the file, so with the file in `~/mira` the path starts with `skua/Scripts`:

```toml
[[account]]
user = "player1"
password = "change-me"
script = "skua/Scripts/Evil/NSoD/VoidAuras/0SmartVoidAuras.cs"
```

From a shell, `mira skua list` prints the full path and the name of each script that matches the words you give it, including `#tag` words. Pass that path to `mira run`:

```sh
mira skua list smart void
mira run -a player1 ~/mira/skua/Scripts/Evil/NSoD/VoidAuras/0SmartVoidAuras.cs
```

## Change script options

In the TUI, select an account that runs a Skua script and press `c`. The **CoreBots** tab edits the CoreBots options of that account. The **Script** tab edits the options of the script, and every account that runs the script shares them. You can change options only while the account is offline or idle.

From a shell, pass options to `mira run` as `--arg Name=value`. For an option outside the `Options` group, write `--arg Group:Name=value`.

## Write your own Skua script

Keep your scripts in any folder outside `~/mira/skua/Scripts`. Their `//cs_include Scripts/...` lines read files from the library:

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

To compile the script without logging in, run `mira check Gold.cs`.

If a folder above the script has its own `CoreBots.cs`, mira reads the includes from that folder instead of the library. A clone of auqw/Scripts works this way.

## Update the library

Press `u` on the Skua tab of the `S` list, or run `mira skua update`. mira downloads the library only when it changed upstream.

An update replaces the whole `~/mira/skua/Scripts` folder, so mira discards any edits you made inside it. If the download fails, mira keeps the current library. Scripts that are running keep running, and the next start uses the new library.

## Update mira

Pull the clone, update the submodule, and install mira again:

```sh
git pull
git submodule update --init --depth 1 skua/vendor/Skua
cargo install --path crates/mira
```

If the worker sources changed, the next Skua script start rebuilds the worker, and the account log shows `building the Skua worker`.

## Find the Skua folder

Run `mira skua path` to print the folder. To move it, set `MIRA_HOME`, which moves the rest of `~/mira` too.

The folder holds the library in `Scripts`, CoreBots and script options in `options`, and compiled scripts in `cache`. mira doesn't read the data folder of Skua itself. To keep options that you set in Skua, copy its `options` folder into `~/mira/skua`.
