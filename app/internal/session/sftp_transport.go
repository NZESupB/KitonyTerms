package session

import (
	"io"
	"sync"
	"time"

	"golang.org/x/crypto/ssh"
)

// 本地管道先解除 SFTP 的读等待；远端拒绝关闭时限时回收 SSH，避免后台协程泄漏。
type sftpTransport struct {
	channel ssh.Channel
	host    *ssh.Client
	reader  *io.PipeReader
	pumps   sync.WaitGroup
	once    sync.Once
}

func newSFTPTransport(host *ssh.Client, channel ssh.Channel) *sftpTransport {
	reader, writer := io.Pipe()
	transport := &sftpTransport{channel: channel, host: host, reader: reader}
	transport.pumps.Add(2)
	go func() {
		defer transport.pumps.Done()
		_, err := io.Copy(writer, channel)
		_ = writer.CloseWithError(err)
	}()
	go func() { defer transport.pumps.Done(); _, _ = io.Copy(io.Discard, channel.Stderr()) }()
	return transport
}
func (transport *sftpTransport) Read(data []byte) (int, error)  { return transport.reader.Read(data) }
func (transport *sftpTransport) Write(data []byte) (int, error) { return transport.channel.Write(data) }
func (transport *sftpTransport) Close() error {
	transport.once.Do(func() {
		_ = transport.reader.Close()
		closed := make(chan struct{})
		go func() { _ = transport.channel.Close(); transport.pumps.Wait(); close(closed) }()
		timer := time.NewTimer(500 * time.Millisecond)
		defer timer.Stop()
		select {
		case <-closed:
		case <-timer.C:
			_ = transport.host.Close()
			<-closed
		}
	})
	return nil
}
