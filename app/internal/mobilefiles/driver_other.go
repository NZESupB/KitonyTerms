//go:build !android || !cgo

package mobilefiles

import "context"

type unsupportedDriver struct{}

func platformDriver() Driver                                           { return unsupportedDriver{} }
func (unsupportedDriver) Available() bool                              { return false }
func (unsupportedDriver) Run(context.Context, Request, Progress) error { return ErrUnavailable }
func (unsupportedDriver) Invalidate(string, context.CancelFunc)        {}
