package main

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"log"
	"net"
	"net/http"
	"os"
	"os/signal"
	"strconv"
	"strings"
	"syscall"
	"time"
)

func main() {
	log.SetOutput(os.Stderr)
	log.SetFlags(log.LstdFlags | log.Lmsgprefix)
	log.SetPrefix("")

	controlAddr := flag.String("control-addr", "127.0.0.1:0", "control HTTP listen address (must be loopback)")
	stateDir := flag.String("state-dir", "", "independent Tailscale state directory")
	hostname := flag.String("hostname", "codeg", "Tailscale hostname for this userspace node")
	authKey := flag.String("auth-key", "", "optional Tailscale auth key")
	controlToken := flag.String("control-token", "", "required token for control API header X-Codeg-Tsnet-Token")
	flag.Parse()

	if strings.TrimSpace(*stateDir) == "" {
		fmt.Fprintln(os.Stderr, "codeg-tsnet: --state-dir is required")
		os.Exit(2)
	}
	if strings.TrimSpace(*controlToken) == "" {
		fmt.Fprintln(os.Stderr, "codeg-tsnet: --control-token is required")
		os.Exit(2)
	}
	if err := os.MkdirAll(*stateDir, 0o700); err != nil {
		fmt.Fprintf(os.Stderr, "codeg-tsnet: create state dir: %v\n", err)
		os.Exit(1)
	}

	host, portStr, err := net.SplitHostPort(*controlAddr)
	if err != nil {
		fmt.Fprintf(os.Stderr, "codeg-tsnet: invalid --control-addr: %v\n", err)
		os.Exit(2)
	}
	if host != "127.0.0.1" && host != "localhost" && host != "::1" {
		fmt.Fprintln(os.Stderr, "codeg-tsnet: --control-addr must bind to loopback")
		os.Exit(2)
	}
	if _, err := strconv.Atoi(portStr); err != nil && portStr != "0" {
		// SplitHostPort already validated ports; keep defensive check.
	}

	mgr := newNodeManager(*hostname, *stateDir, *authKey)
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	ctrl := &controlServer{mgr: mgr}
	httpSrv := &http.Server{
		Handler: withToken(*controlToken, ctrl.routes()),
	}
	ctrl.shutdown = func() {
		stop()
		shCtx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
		defer cancel()
		_ = httpSrv.Shutdown(shCtx)
	}

	ln, err := net.Listen("tcp", *controlAddr)
	if err != nil {
		fmt.Fprintf(os.Stderr, "codeg-tsnet: listen control: %v\n", err)
		os.Exit(1)
	}

	boot := bootstrapInfo{
		ControlAddr: ln.Addr().String(),
		PID:         os.Getpid(),
	}
	if err := json.NewEncoder(os.Stdout).Encode(boot); err != nil {
		fmt.Fprintf(os.Stderr, "codeg-tsnet: bootstrap write: %v\n", err)
		os.Exit(1)
	}
	_ = os.Stdout.Sync()

	go func() {
		if err := httpSrv.Serve(ln); err != nil && !errors.Is(err, http.ErrServerClosed) {
			log.Printf("control server error: %v", err)
			stop()
		}
	}()

	<-ctx.Done()
	mgr.shutdown()
	shCtx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	_ = httpSrv.Shutdown(shCtx)
}
