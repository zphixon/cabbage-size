# cabbage size

get the size of your ~~cabbage~~ anything

## api

deprecated legacy api:

```shell
$ curl localhost:12002/cs
38
```

new api below.

### get the size of something

`GET /size?viewer=<viewer>&streamer=<streamer>`

required parameters:

* `streamer` - login ID of the streamer (basically the username, different from display name)
* `viewer` - login ID of the viewer (basically the username, different from display name)

optional parameters:

* `time_limit` - required time in seconds before new size requests will be allowed for that viewer

simple example:

```shell
$ curl -sX GET localhost:12002/size?viewer=teej_dv&streamer=theprimeagen
{"size":14,"is_message":false,"message":""}
```

example with time limit:

```shell
$ curl -sX GET localhost:12002/size?viewer=teej_dv&streamer=theprimeagen&time_limit=600
{"size":3,"is_message":false,"message":""}
$ # instantly
$ curl -sX GET localhost:12002/size?viewer=teej_dv&streamer=theprimeagen&time_limit=600
{"size":3,"is_message":false,"message":""}
$ # 10 minutes later
$ curl -sX GET localhost:12002/size?viewer=teej_dv&streamer=theprimeagen&time_limit=600
{"size":99,"is_message":false,"message":""}
```

### change the upper and lower bounds

`PUT /reset?streamer=<streamer>`

required parameters:

* `streamer` - login ID of the streamer
  
optional parameters:

* `upper` - integer for the upper bound. default 100
* `lower` - integer for the lower bound. default 1

example:

```shell
$ curl -sX PUT localhost:12002/reset?streamer=theprimeagen
{ "size":0,"is_message":true,"message"="reset size"}
```

## response

for publicly documented parts of the new api, the response is JSON with three
fields - `size`, `is_message`, and `message`.

if `is_message` is true, `size` contains the http status code. this is to
work around the fact that nightbot doesn't include status codes in their custom
api command language thingy. if querying the `/reset` endpoint, `size` is 0.
otherwise, `size` is the size.

example for `/size` endpoint:

```json
{
  "size": 17,
  "is_message": false,
  "message": ""
}
```
