package main

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"net"
	"os"
	"os/signal"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/emiago/sipgo"
	"github.com/emiago/sipgo/sip"
)

const (
	readySchema   = "sipx.comparative-load.ready.v1"
	summarySchema = "sipx.comparative-load.responder.v1"
	maxEvents     = 65_536
	maxLogBytes   = 16 * 1024 * 1024
)

type counters struct {
	mu sync.Mutex

	active          map[string]struct{}
	activeHighWater int
	invites         int
	completed       int
	refused         int
	invalid         int
	internalErrors  int
}

func newCounters() *counters {
	return &counters{active: make(map[string]struct{})}
}

func (c *counters) admit(callID string, maximum int) bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	if _, exists := c.active[callID]; exists {
		return true
	}
	if len(c.active) >= maximum {
		c.refused++
		return false
	}
	c.active[callID] = struct{}{}
	c.invites++
	if len(c.active) > c.activeHighWater {
		c.activeHighWater = len(c.active)
	}
	return true
}

func (c *counters) finish(callID string) bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	if _, exists := c.active[callID]; !exists {
		return false
	}
	delete(c.active, callID)
	c.completed++
	return true
}

func (c *counters) countInvalid() {
	c.mu.Lock()
	c.invalid++
	c.mu.Unlock()
}

func (c *counters) countInternalError() {
	c.mu.Lock()
	c.internalErrors++
	c.mu.Unlock()
}

func (c *counters) snapshot() map[string]any {
	c.mu.Lock()
	defer c.mu.Unlock()
	return map[string]any{
		"counts": map[string]int{
			"active_high_water": c.activeHighWater,
			"completed":         c.completed,
			"internal_errors":   c.internalErrors,
			"invalid":           c.invalid,
			"invites":           c.invites,
			"refused":           c.refused,
		},
		"post_drain": map[string]int{
			"active_dialogs":        len(c.active),
			"dispatcher_routes":     0,
			"endpoint_transactions": 0,
			"owned_tasks":           0,
		},
		"schema": summarySchema,
	}
}

func callID(req *sip.Request) (string, bool) {
	header := req.CallID()
	if header == nil {
		return "", false
	}
	value := header.Value()
	return value, value != ""
}

func deterministicTag(seed uint64, value string) (string, bool) {
	if !strings.HasPrefix(value, "cl-") || !strings.HasSuffix(value, "@driver.invalid") {
		return "", false
	}
	body := strings.TrimSuffix(strings.TrimPrefix(value, "cl-"), "@driver.invalid")
	separator := strings.LastIndexByte(body, '-')
	if separator <= 0 || separator == len(body)-1 {
		return "", false
	}
	runID := body[:separator]
	index := body[separator+1:]
	if len(runID) != 32 {
		return "", false
	}
	if _, err := hex.DecodeString(runID); err != nil {
		return "", false
	}
	if _, err := strconv.ParseUint(index, 10, 64); err != nil {
		return "", false
	}
	material := strings.Join([]string{strconv.FormatUint(seed, 10), runID, index, "to"}, "\x00")
	digest := sha256.Sum256([]byte(material))
	return "t-" + hex.EncodeToString(digest[:])[:16], true
}

func response(req *sip.Request, status int, reason string) *sip.Response {
	return sip.NewResponseFromRequest(req, status, reason, nil)
}

func successfulInvite(req *sip.Request, seed uint64, address string) (*sip.Response, bool) {
	value, ok := callID(req)
	if !ok {
		return nil, false
	}
	tag, ok := deterministicTag(seed, value)
	if !ok {
		return nil, false
	}
	host, portText, err := net.SplitHostPort(address)
	if err != nil {
		return nil, false
	}
	port, err := strconv.Atoi(portText)
	if err != nil {
		return nil, false
	}
	res := response(req, sip.StatusOK, "OK")
	to := res.To()
	if to == nil {
		return nil, false
	}
	to.Params.Remove("tag")
	to.Params.Add("tag", tag)
	res.AppendHeader(&sip.ContactHeader{
		Address: sip.Uri{Scheme: "sip", User: "load", Host: host, Port: port},
	})
	return res, true
}

func emit(value any) error {
	encoded, err := json.Marshal(value)
	if err != nil {
		return err
	}
	_, err = fmt.Fprintln(os.Stdout, string(encoded))
	return err
}

func run() error {
	local := flag.String("local", "127.0.0.1:0", "UDP listen address")
	maximum := flag.Int("max-active", 2048, "maximum active dialogs")
	duration := flag.Int("duration", 180, "hard process lifetime in seconds")
	cleanup := flag.Int("cleanup", 5, "bounded shutdown allowance in seconds")
	seed := flag.Uint64("seed", 7, "deterministic profile seed")
	flag.Parse()
	if *maximum <= 0 || *duration <= 0 || *cleanup <= 0 {
		return errors.New("max-active, duration and cleanup must be positive")
	}

	base, cancelLifetime := context.WithTimeout(context.Background(), time.Duration(*duration)*time.Second)
	defer cancelLifetime()
	ctx, stopSignal := signal.NotifyContext(base, os.Interrupt, syscall.SIGTERM)
	defer stopSignal()

	ua, err := sipgo.NewUA(sipgo.WithUserAgent("comparative-load-peer"))
	if err != nil {
		return err
	}
	server, err := sipgo.NewServer(ua)
	if err != nil {
		_ = ua.Close()
		return err
	}

	state := newCounters()
	var readyAddress string
	var readyMu sync.RWMutex
	server.OnInvite(func(req *sip.Request, tx sip.ServerTransaction) {
		value, ok := callID(req)
		if !ok {
			state.countInvalid()
			_ = tx.Respond(response(req, sip.StatusBadRequest, "Bad Request"))
			return
		}
		if !state.admit(value, *maximum) {
			_ = tx.Respond(response(req, sip.StatusServiceUnavailable, "Service Unavailable"))
			return
		}
		if err := tx.Respond(response(req, sip.StatusTrying, "Trying")); err != nil {
			state.countInternalError()
			return
		}
		readyMu.RLock()
		address := readyAddress
		readyMu.RUnlock()
		final, valid := successfulInvite(req, *seed, address)
		if !valid {
			state.countInvalid()
			_ = tx.Respond(response(req, sip.StatusBadRequest, "Bad Request"))
			return
		}
		if err := tx.Respond(final); err != nil {
			state.countInternalError()
		}
	})
	server.OnAck(func(req *sip.Request, tx sip.ServerTransaction) {})
	server.OnBye(func(req *sip.Request, tx sip.ServerTransaction) {
		value, ok := callID(req)
		if !ok || !state.finish(value) {
			state.countInvalid()
			_ = tx.Respond(response(req, sip.StatusCallTransactionDoesNotExists, "Call Does Not Exist"))
			return
		}
		if err := tx.Respond(response(req, sip.StatusOK, "OK")); err != nil {
			state.countInternalError()
		}
	})

	ready := make(chan error, 1)
	readyContext := context.WithValue(ctx, sipgo.ListenReadyCtxKey, sipgo.ListenReadyFuncCtxValue(
		func(network, address string) {
			readyMu.Lock()
			readyAddress = address
			readyMu.Unlock()
			ready <- emit(map[string]any{
				"address": address,
				"limits": map[string]int{
					"active":       *maximum,
					"events":       maxEvents,
					"stderr_bytes": maxLogBytes,
					"stdout_bytes": maxLogBytes,
				},
				"pid":       os.Getpid(),
				"role":      "responder",
				"schema":    readySchema,
				"transport": network,
			})
		},
	))
	serveDone := make(chan error, 1)
	go func() {
		serveDone <- server.ListenAndServe(readyContext, "udp", *local)
	}()
	select {
	case err := <-ready:
		if err != nil {
			stopSignal()
			_ = ua.Close()
			return err
		}
	case err := <-serveDone:
		stopSignal()
		_ = ua.Close()
		if err != nil {
			return fmt.Errorf("responder stopped before readiness: %w", err)
		}
		return errors.New("responder stopped before readiness")
	case <-ctx.Done():
		_ = ua.Close()
		return ctx.Err()
	}

	serveErr := <-serveDone
	shutdownDone := make(chan error, 1)
	go func() { shutdownDone <- ua.Close() }()
	select {
	case err := <-shutdownDone:
		if err != nil {
			return err
		}
	case <-time.After(time.Duration(*cleanup) * time.Second):
		return errors.New("bounded shutdown expired")
	}
	if serveErr != nil && ctx.Err() == nil {
		return serveErr
	}
	return emit(state.snapshot())
}

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
