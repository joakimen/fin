package todoist

import (
	"net/http"
	"time"

	"github.com/joakimen/fin/config"
	"github.com/joakimen/fin/internal"
	"github.com/joakimen/fin/task"
)

// ActivitiesPayload represents the root structure of the Todoist activity log response.
type ActivitiesPayload struct {
	Activities []Activity `json:"events"`
	Count      int        `json:"count"`
}

// Activity represents a single event from the Todoist activity log.
// They may be of different types, such as  comments tasks.
// We are only interested in completed (EventType) tasks (ObjectType).
type Activity struct {
	ID         int       `json:"id"`
	ObjectType string    `json:"object_type"`
	ObjectID   string    `json:"object_id"`
	EventType  string    `json:"event_type"`
	EventDate  time.Time `json:"event_date"`
	ExtraData  struct {
		Content   string `json:"content"`
		NoteCount int    `json:"note_count"`
	} `json:"extra_data"`
}

func deserialize(body []byte) ActivitiesPayload {
	return internal.Deserialize[ActivitiesPayload](body)
}

func getActivity(cfg *config.Config) []byte {
	apiURL := "https://api.todoist.com/sync/v9/activity/get"
	client := internal.NewClient()
	req := internal.NewRequest(http.MethodGet, apiURL)
	req.Header.Set("Authorization", "Bearer "+cfg.Todoist.APIToken)
	req.Header.Set("Accept", "application/json")

	queryParams := req.URL.Query()
	queryParams.Add("object_type", "item")
	queryParams.Add("event_type", "completed")
	queryParams.Add("limit", "100") // default 30
	req.URL.RawQuery = queryParams.Encode()

	resp := internal.DoRequest(client, req)
	return resp
}

func toTask(activity *Activity) task.Task {
	return task.Task{
		Source:      task.Todoist,
		CompletedAt: activity.EventDate,
		Title:       activity.ExtraData.Content,
	}
}

func toTasks(activities *[]Activity) []task.Task {
	tasks := make([]task.Task, 0, len(*activities))
	for _, activity := range *activities {
		tasks = append(tasks, toTask(&activity))
	}
	return tasks
}

// GetCompletedTasks queries Activity data from Todoist, and returns them as a generic Task slice.
func GetCompletedTasks(cfg *config.Config) []task.Task {
	resp := getActivity(cfg)

	if cfg.SaveTestData {
		internal.WriteTestData("todoist.json", resp)
	}

	activityPayload := deserialize(resp)
	return toTasks(&activityPayload.Activities)
}
