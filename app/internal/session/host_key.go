package session

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net"
	"time"

	"golang.org/x/crypto/ssh"
	"kitonyterms/internal/model"
)

type hostKeyChangedError struct {
	expected  string
	actual    string
	algorithm string
}

func (e *hostKeyChangedError) Error() string {
	return fmt.Sprintf("主机密钥变更: 原 %s，现 %s", e.expected, e.actual)
}

type handshakeDialError struct{ cause error }

func (e *handshakeDialError) Error() string { return e.cause.Error() }
func (e *handshakeDialError) Unwrap() error { return e.cause }

func (m *Manager) handshake(s *Session, p model.Profile, dial func(context.Context, string) (net.Conn, error)) (*ssh.Client, []io.Closer, error) {
	ctx, cancel := context.WithTimeout(s.ctx, 120*time.Second)
	defer cancel()
	return m.handshakeContext(ctx, s, p, dial)
}

func (m *Manager) handshakeContext(ctx context.Context, s *Session, p model.Profile, dial func(context.Context, string) (net.Conn, error)) (*ssh.Client, []io.Closer, error) {
	var algorithms []string
	var changedErr error
	for {
		client, closers, err := m.handshakeAttempt(ctx, s, p, dial, algorithms)
		if err == nil {
			return client, closers, nil
		}
		if ctx.Err() != nil {
			return nil, nil, ctx.Err()
		}
		var dialErr *handshakeDialError
		if errors.As(err, &dialErr) {
			return nil, nil, err
		}
		var changed *hostKeyChangedError
		if errors.As(err, &changed) {
			changedErr = err
			algorithms = remainingHostKeyAlgorithms(algorithms, changed.algorithm)
			if len(algorithms) == 0 {
				return nil, nil, err
			}
			continue
		}
		var negotiation *ssh.AlgorithmNegotiationError
		if changedErr != nil && errors.As(err, &negotiation) && negotiation.What == "host key" {
			return nil, nil, changedErr
		}
		return nil, nil, err
	}
}

// 旧信任记录只有指纹，协商首选变化时继续寻找原密钥；不增加弱签名算法。
// 每轮排除一种密钥及其签名别名，尝试次数有界，所有尝试共用原超时与取消。
func remainingHostKeyAlgorithms(previous []string, rejected string) []string {
	candidates := previous
	if candidates == nil {
		candidates = ssh.SupportedAlgorithms().HostKeys
	}
	remaining := make([]string, 0, len(candidates))
	for _, algorithm := range candidates {
		keyType := algorithm
		switch algorithm {
		case ssh.KeyAlgoRSASHA256, ssh.KeyAlgoRSASHA512:
			keyType = ssh.KeyAlgoRSA
		case ssh.CertAlgoRSASHA256v01, ssh.CertAlgoRSASHA512v01:
			keyType = ssh.CertAlgoRSAv01
		}
		if keyType != rejected {
			remaining = append(remaining, algorithm)
		}
	}
	if previous != nil && len(remaining) == len(previous) {
		return nil
	}
	return remaining
}
