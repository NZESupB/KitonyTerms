package session

import (
	"bytes"
	"context"
	"fmt"
	"time"

	"kitonyterms/internal/shellintegration"
)

type shellObservation struct {
	command string
	err     error
}
type shellEvent struct {
	kind      byte
	data      []byte
	directory string
	answer    chan shellObservation
}
type shellIO struct {
	ctx      context.Context
	output   *outputBuffer
	observer *shellintegration.Observer
	events   chan shellEvent
	done     chan struct{}
	failure  error
}

func newShellIO(ctx context.Context, output *outputBuffer, observer *shellintegration.Observer) *shellIO {
	runtime := &shellIO{ctx: ctx, output: output, observer: observer, events: make(chan shellEvent, 8), done: make(chan struct{})}
	go runtime.run()
	return runtime
}
func (r *shellIO) closedError() error {
	if r.failure != nil {
		return r.failure
	}
	return fmt.Errorf("终端输出处理已结束")
}
func (r *shellIO) send(event shellEvent) error {
	select {
	case <-r.done:
		return r.closedError()
	case <-r.ctx.Done():
		return r.ctx.Err()
	case r.events <- event:
		return nil
	}
}
func (r *shellIO) outputData(data []byte, stderr bool) error {
	kind := byte('o')
	if stderr {
		kind = 'e'
	}
	return r.send(shellEvent{kind: kind, data: bytes.Clone(data)})
}
func (r *shellIO) observe(data []byte, directory string) (string, error) {
	kind := byte('i')
	if directory != "" {
		kind = 'd'
	}
	answer := make(chan shellObservation, 1)
	if err := r.send(shellEvent{kind: kind, data: data, directory: directory, answer: answer}); err != nil {
		return "", err
	}
	select {
	case value := <-answer:
		return value.command, value.err
	case <-r.done:
		return "", r.closedError()
	case <-r.ctx.Done():
		return "", r.ctx.Err()
	}
}
func (r *shellIO) finish() error {
	err := r.send(shellEvent{kind: 'f'})
	<-r.done
	if r.failure != nil {
		return r.failure
	}
	return err
}
func (r *shellIO) append(data []byte) error {
	if len(data) == 0 {
		return nil
	}
	return r.output.append(r.ctx, data)
}

func (r *shellIO) run() {
	defer close(r.done)
	var filter shellintegration.BootstrapFilter
	filter.Start(time.Now())
	timer := time.NewTimer(3 * time.Second)
	defer timer.Stop()
	deadline := timer.C
	for {
		select {
		case <-r.ctx.Done():
			r.failure = r.ctx.Err()
			return
		case <-deadline:
			deadline = nil
			if r.failure = r.append(filter.Finish()); r.failure != nil {
				return
			}
		case event := <-r.events:
			var visible []byte
			switch event.kind {
			case 'o':
				r.observer.Output(event.data, true, time.Now())
				visible = filter.Filter(event.data, time.Now())
			case 'e':
				r.observer.Output(event.data, false, time.Now())
				visible = event.data
			case 'i', 'd':
				var observation shellObservation
				if event.kind == 'd' {
					observation.err = r.observer.Directory(event.directory, time.Now())
				} else {
					observation.command = r.observer.Input(event.data)
				}
				if observation.err == nil && (len(event.data) > 0 || event.kind == 'd') {
					observation.err = r.append(filter.Finish())
				}
				event.answer <- observation
				if observation.err != nil && event.kind != 'd' {
					r.failure = observation.err
					return
				}
			case 'f':
				r.failure = r.append(filter.Finish())
				return
			}
			if r.failure = r.append(visible); r.failure != nil {
				return
			}
			if !filter.Active() && deadline != nil {
				timer.Stop()
				deadline = nil
			}
		}
	}
}
