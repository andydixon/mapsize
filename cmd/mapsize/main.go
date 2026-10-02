// Command mapsize is an interactive terminal disk usage analyser.
package main

import (
	"flag"
	"fmt"
	"io"
	"log/slog"
	"os"
	"runtime"
	"runtime/pprof"
	"strings"

	"github.com/andydixon/mapsize/internal/app"
	"github.com/andydixon/mapsize/internal/brand"
	"github.com/andydixon/mapsize/internal/config"
	"github.com/andydixon/mapsize/internal/inventory"
	"github.com/andydixon/mapsize/internal/scan"
	"github.com/andydixon/mapsize/internal/snapshot"
	"github.com/andydixon/mapsize/internal/textutil"
)

type multiFlag []string

func (m *multiFlag) String() string     { return strings.Join(*m, ",") }
func (m *multiFlag) Set(s string) error { *m = append(*m, s); return nil }

func main() {
	os.Exit(run(os.Args[1:]))
}

func run(args []string) int {
	settings, cfgErr := config.Load()
	fs := flag.NewFlagSet(brand.Name, flag.ContinueOnError)
	fs.Usage = func() { usage(fs) }
	var (
		excludes   multiFlag
		workers    = fs.String("workers", settings.Workers, "concurrency: conservative, balanced, aggressive or a number")
		follow     = fs.String("follow-symlinks", settings.FollowSymlinks, "none, same-filesystem or all")
		oneFS      = fs.Bool("one-file-system", settings.OneFileSystem, "do not cross filesystem boundaries")
		oneFSx     = fs.Bool("x", false, "short for --one-file-system")
		readOnly   = fs.Bool("read-only", false, "disable all modifying actions")
		noUI       = fs.Bool("no-ui", false, "scan without the interactive interface and print a summary")
		jsonOut    = fs.Bool("json", false, "print the inventory as JSON")
		csvOut     = fs.Bool("csv", false, "print the inventory as CSV")
		depth      = fs.Int("depth", -1, "maximum depth for --json/--csv (-1 = unlimited)")
		top        = fs.Int("top", 0, "print the N largest entries of the root")
		largest    = fs.Int("largest-files", 0, "print the N largest files")
		dups       = fs.Bool("duplicates", false, "find duplicate files (non-interactive)")
		save       = fs.String("save", "", "save a snapshot to `FILE` after scanning")
		snap       = fs.String("snapshot", "", "alias for --save")
		load       = fs.String("load", "", "open a saved snapshot `FILE` instead of scanning")
		scanPath   = fs.String("scan", "", "path to scan (alternative to a positional argument)")
		compare    = fs.Bool("compare", false, "compare two snapshots: --compare OLD NEW")
		si         = fs.Bool("si", settings.Units == "si", "use SI units (kB, MB) instead of IEC (KiB, MiB)")
		logical    = fs.Bool("apparent", settings.SizeMode == "logical", "size by logical (apparent) size instead of disk usage")
		theme      = fs.String("theme", settings.Theme, "colour theme: default, dark, high-contrast, mono")
		colorMode  = fs.String("color", "auto", "colour support: auto, truecolor, 256, 16, none")
		noMouse    = fs.Bool("no-mouse", !settings.Mouse, "disable mouse support")
		logFile    = fs.String("log", "", "write a debug log to `FILE`")
		logLevel   = fs.String("log-level", "info", "log level: error, warn, info, debug, trace")
		cpuProfile = fs.String("cpuprofile", "", "write a CPU profile to `FILE`")
		memProfile = fs.String("memprofile", "", "write a heap profile to `FILE` on exit")
		version    = fs.Bool("version", false, "print version and exit")
	)
	fs.Var(&excludes, "exclude", "exclude `PATTERN` (glob on name, or path prefix if it contains a separator); repeatable")

	var positional []string
	for {
		if err := fs.Parse(args); err != nil {
			if err == flag.ErrHelp {
				return 0
			}
			return 2
		}
		args = fs.Args()
		if len(args) == 0 {
			break
		}
		positional = append(positional, args[0])
		args = args[1:]
	}
	if *version {
		fmt.Printf("%s %s (%s/%s)\n", brand.Name, brand.Version, runtime.GOOS, runtime.GOARCH)
		return 0
	}
	if cfgErr != nil {
		fmt.Fprintf(os.Stderr, "%s: warning: config: %v (using defaults)\n", brand.Name, cfgErr)
	}
	textutil.SI = *si

	if *logFile != "" {
		closeLog, err := setupLog(*logFile, *logLevel)
		if err != nil {
			fmt.Fprintf(os.Stderr, "%s: %v\n", brand.Name, err)
			return 2
		}
		defer closeLog()
	} else {
		slog.SetDefault(slog.New(slog.NewTextHandler(io.Discard, nil)))
	}
	if *cpuProfile != "" {
		f, err := os.Create(*cpuProfile)
		if err == nil {
			pprof.StartCPUProfile(f)
			defer pprof.StopCPUProfile()
		}
	}
	if *memProfile != "" {
		defer func() {
			if f, err := os.Create(*memProfile); err == nil {
				runtime.GC()
				pprof.WriteHeapProfile(f)
				f.Close()
			}
		}()
	}

	cfg := &app.Config{
		NoUI: *noUI, JSON: *jsonOut, CSV: *csvOut, Depth: *depth, Top: *top, LargestFiles: *largest,
		Duplicates: *dups, ReadOnly: *readOnly, Theme: *theme, Mouse: !*noMouse, Color: *colorMode, Load: *load,
		Settings: settings,
	}
	if *logical {
		cfg.SizeMode = inventory.SizeLogical
	}
	cfg.Save = *save
	if cfg.Save == "" {
		cfg.Save = *snap
	}
	if *scanPath != "" {
		positional = append([]string{*scanPath}, positional...)
	}
	if *compare {
		if len(positional) != 2 {
			fmt.Fprintf(os.Stderr, "%s: --compare needs exactly two snapshot files\n", brand.Name)
			return 2
		}
		cfg.Compare = [2]string{positional[0], positional[1]}
	} else {
		if len(positional) > 1 {
			fmt.Fprintf(os.Stderr, "%s: only one path may be scanned at a time\n", brand.Name)
			return 2
		}
		if len(positional) == 1 && cfg.Load == "" && snapshot.IsSnapshot(positional[0]) {
			cfg.Load = positional[0]
		} else {
			cfg.Paths = positional
		}
	}

	w, err := scan.Workers(*workers)
	if err != nil {
		fmt.Fprintf(os.Stderr, "%s: %v\n", brand.Name, err)
		return 2
	}
	fm, err := scan.ParseFollow(*follow)
	if err != nil {
		fmt.Fprintf(os.Stderr, "%s: %v\n", brand.Name, err)
		return 2
	}
	cfg.ScanOpts = scan.Options{Workers: w, Follow: fm, OneFileSystem: *oneFS || *oneFSx,
		Excludes: append(append([]string(nil), settings.Excludes...), excludes...)}

	if err := app.Run(cfg); err != nil {
		fmt.Fprintf(os.Stderr, "%s: %v\n", brand.Name, textutil.Sanitize(err.Error()))
		return 1
	}
	return 0
}

func setupLog(path, level string) (func(), error) {
	lv := map[string]slog.Level{"error": slog.LevelError, "warn": slog.LevelWarn, "info": slog.LevelInfo,
		"debug": slog.LevelDebug, "trace": slog.LevelDebug - 4}
	l, ok := lv[level]
	if !ok {
		return nil, fmt.Errorf("invalid --log-level %q", level)
	}
	f, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
	if err != nil {
		return nil, err
	}
	slog.SetDefault(slog.New(slog.NewTextHandler(f, &slog.HandlerOptions{Level: l})))
	return func() { f.Close() }, nil
}

func usage(fs *flag.FlagSet) {
	w := fs.Output()
	fmt.Fprintf(w, `%[1]s — interactive terminal disk usage analyser

Usage:
  %[1]s [flags] [PATH]             scan PATH (default .) interactively
  %[1]s [flags] SNAPSHOT%[2]s        open a saved snapshot
  %[1]s --compare OLD NEW          compare two snapshots
  %[1]s PATH --top 50 | --json | --csv | --no-ui --save FILE

Flags:
`, brand.Name, brand.SnapshotExt)
	fs.PrintDefaults()
}
