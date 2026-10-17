#define _POSIX_C_SOURCE 200809L
/* Feasibility probe, not production clock correlation. No physical MIDI or
 * audio endpoint is opened. All events stay on this process's virtual ports. */
#include <alsa/asoundlib.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>

static void check(int result, const char *operation) {
    if (result < 0) { fprintf(stderr, "%s: %s\n", operation, snd_strerror(result)); exit(1); }
}
static int64_t mono(void) {
    struct timespec now;
    if (clock_gettime(CLOCK_MONOTONIC, &now)) { perror("clock_gettime"); exit(1); }
    return (int64_t)now.tv_sec * 1000000000 + now.tv_nsec;
}
static int64_t real_ns(const snd_seq_real_time_t *now) {
    return (int64_t)now->tv_sec * 1000000000 + now->tv_nsec;
}
static void delay(long nanoseconds) {
    struct timespec remaining = {0, nanoseconds};
    while (nanosleep(&remaining, &remaining) < 0 && errno == EINTR) {}
}
static int64_t sample(snd_seq_t *seq, int queue, snd_seq_queue_status_t *status) {
    int64_t before = mono();
    check(snd_seq_get_queue_status(seq, queue, status), "queue status");
    int64_t after = mono();
    int64_t time = real_ns(snd_seq_queue_status_get_real_time(status));
    printf("sample,%" PRId64 ",%" PRId64 ",%" PRId64 ",%d\n", before, time, after, snd_seq_queue_status_get_status(status));
    return time;
}
int main(void) {
    snd_seq_t *seq, *observer;
    check(snd_seq_open(&seq, "default", SND_SEQ_OPEN_DUPLEX, SND_SEQ_NONBLOCK), "open");
    check(snd_seq_set_client_name(seq, "Crest clock feasibility"), "client name");
    check(snd_seq_open(&observer, "default", SND_SEQ_OPEN_DUPLEX, SND_SEQ_NONBLOCK), "observer");
    int source = snd_seq_create_simple_port(observer, "Crest probe source",
        SND_SEQ_PORT_CAP_READ, SND_SEQ_PORT_TYPE_MIDI_GENERIC);
    check(source, "source port");
    int queue = snd_seq_alloc_named_queue(seq, "Crest clock probe");
    check(queue, "queue");
    snd_seq_queue_status_t *status;
    check(snd_seq_queue_status_malloc(&status), "status allocation");
    snd_seq_queue_timer_t *timer;
    check(snd_seq_queue_timer_malloc(&timer), "timer allocation");
    check(snd_seq_get_queue_timer(observer, queue, timer), "cross-client timer");
    snd_timer_id_t *id;
    check(snd_timer_id_malloc(&id), "timer ID allocation");
    snd_timer_id_copy(id, snd_seq_queue_timer_get_id(timer));
    printf("timer,%d,%d,%d,%d,%d\n", snd_seq_queue_timer_get_type(timer), snd_timer_id_get_class(id), snd_timer_id_get_device(id), snd_timer_id_get_subdevice(id), snd_seq_queue_timer_get_resolution(timer));
    snd_seq_port_info_t *port;
    check(snd_seq_port_info_malloc(&port), "port allocation");
    snd_seq_port_info_set_name(port, "Crest timestamp probe");
    snd_seq_port_info_set_capability(port, SND_SEQ_PORT_CAP_WRITE | SND_SEQ_PORT_CAP_SUBS_WRITE);
    snd_seq_port_info_set_type(port, SND_SEQ_PORT_TYPE_MIDI_GENERIC | SND_SEQ_PORT_TYPE_APPLICATION);
    snd_seq_port_info_set_timestamping(port, 1);
    snd_seq_port_info_set_timestamp_real(port, 1);
    snd_seq_port_info_set_timestamp_queue(port, queue);
    check(snd_seq_create_port(seq, port), "input port");
    int input = snd_seq_port_info_get_port(port);
    check(snd_seq_get_any_port_info(observer, snd_seq_client_id(seq), input, port), "cross-client port");
    printf("port,%d,%d,%d\n", snd_seq_port_info_get_timestamping(port), snd_seq_port_info_get_timestamp_real(port), snd_seq_port_info_get_timestamp_queue(port));
    check(snd_seq_start_queue(seq, queue, NULL), "start");
    check(snd_seq_drain_output(seq), "drain");
    for (int i = 0; i < 200; i++) { sample(observer, queue, status); delay(5000000); }
    snd_seq_event_t note;
    snd_seq_ev_clear(&note);
    snd_seq_ev_set_noteon(&note, 1, 60, 100);
    snd_seq_ev_set_source(&note, source);
    snd_seq_ev_set_dest(&note, snd_seq_client_id(seq), input);
    snd_seq_ev_set_direct(&note);
    int64_t before = sample(observer, queue, status);
    check(snd_seq_event_output_direct(observer, &note), "send");
    int64_t after = sample(observer, queue, status);
    delay(30000000);
    snd_seq_event_t *received;
    check(snd_seq_event_input(seq, &received), "receive");
    printf("event,%" PRId64 ",%" PRId64 ",%" PRId64 ",%" PRId64 ",%d,%d\n", before, real_ns(&received->time.time), after, mono(), received->flags, received->queue);
    if (received->type != SND_SEQ_EVENT_NOTEON || received->data.note.note != 60
        || received->data.note.velocity != 100 || received->data.note.channel != 1
        || (received->flags & SND_SEQ_TIME_STAMP_MASK) != SND_SEQ_TIME_STAMP_REAL
        || received->queue != queue) {
        fprintf(stderr, "Kernel loopback changed note or timestamp ownership\n");
        return 1;
    }
    check(snd_seq_start_queue(seq, queue, NULL), "restart");
    check(snd_seq_drain_output(seq), "drain restart");
    sample(observer, queue, status);
    check(snd_seq_free_queue(seq, queue), "free queue");
    snd_seq_port_info_free(port);
    snd_timer_id_free(id);
    snd_seq_queue_timer_free(timer);
    snd_seq_queue_status_free(status);
    snd_seq_close(observer);
    snd_seq_close(seq);
    return 0;
}
