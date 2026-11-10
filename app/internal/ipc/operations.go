package ipc

import (
	"context"
	"kitonyterms/internal/model"
)

func (b *Backend) ExecuteOperation(ctx context.Context, id, generation string, request model.OperationRequest) (model.OperationReply, error) {
	if err := b.check(); err != nil {
		return model.OperationReply{}, err
	}
	return b.manager.ExecuteOperation(ctx, id, generation, request)
}
func (b *Backend) GetOperationChallenges(id, generation string) ([]model.OperationChallenge, error) {
	if err := b.check(); err != nil {
		return nil, err
	}
	return b.manager.OperationChallenges(id, generation)
}
func (b *Backend) AnswerOperation(answer model.OperationAnswer) error {
	if err := b.check(); err != nil {
		return err
	}
	return b.manager.AnswerOperation(answer)
}
func (b *Backend) CancelOperation(id, generation, operationID string) error {
	if err := b.check(); err != nil {
		return err
	}
	return b.manager.CancelOperation(id, generation, operationID)
}
