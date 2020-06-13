echo "Resetting cabbage size records"

./stop.sh

echo '1' > lower_cabbage_size
echo '100' > upper_cabbage_size

./start.sh
